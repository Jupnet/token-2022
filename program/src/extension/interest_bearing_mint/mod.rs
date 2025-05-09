use ethnum::{AsU256, U256};
#[cfg(feature = "serde-traits")]
use serde::{Deserialize, Serialize};
use {
    crate::{
        extension::{Extension, ExtensionType},
        trim_ui_amount_string,
    },
    bytemuck::{Pod, Zeroable},
    solana_program::program_error::ProgramError,
    spl_pod::{
        optional_keys::OptionalNonZeroPubkey,
        primitives::{PodI16, PodI64},
    },
    std::convert::TryInto,
};

/// Interest-bearing mint extension instructions
pub mod instruction;

/// Interest-bearing mint extension processor
pub mod processor;

/// Annual interest rate, expressed as basis points
pub type BasisPoints = PodI16;
const ONE_IN_BASIS_POINTS: f64 = 10_000.;
const SECONDS_PER_YEAR: f64 = 60. * 60. * 24. * 365.24;

/// `UnixTimestamp` expressed with an alignment-independent type
pub type UnixTimestamp = PodI64;

/// Interest-bearing extension data for mints
///
/// Tokens accrue interest at an annual rate expressed by `current_rate`,
/// compounded continuously, so APY will be higher than the published interest
/// rate.
///
/// To support changing the rate, the config also maintains state for the
/// previous rate.
#[repr(C)]
#[cfg_attr(feature = "serde-traits", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde-traits", serde(rename_all = "camelCase"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct InterestBearingConfig {
    /// Authority that can set the interest rate and authority
    pub rate_authority: OptionalNonZeroPubkey,
    /// Timestamp of initialization, from which to base interest calculations
    pub initialization_timestamp: UnixTimestamp,
    /// Average rate from initialization until the last time it was updated
    pub pre_update_average_rate: BasisPoints,
    /// Timestamp of the last update, used to calculate the total amount accrued
    pub last_update_timestamp: UnixTimestamp,
    /// Current rate, since the last update
    pub current_rate: BasisPoints,
}
impl InterestBearingConfig {
    fn pre_update_timespan(&self) -> Option<i64> {
        i64::from(self.last_update_timestamp).checked_sub(self.initialization_timestamp.into())
    }

    fn pre_update_exp(&self) -> Option<f64> {
        let numerator = (i16::from(self.pre_update_average_rate) as i128)
            .checked_mul(self.pre_update_timespan()? as i128)? as f64;
        let exponent = numerator / SECONDS_PER_YEAR / ONE_IN_BASIS_POINTS;
        Some(exponent.exp())
    }

    fn post_update_timespan(&self, unix_timestamp: i64) -> Option<i64> {
        unix_timestamp.checked_sub(self.last_update_timestamp.into())
    }

    fn post_update_exp(&self, unix_timestamp: i64) -> Option<f64> {
        let numerator = (i16::from(self.current_rate) as i128)
            .checked_mul(self.post_update_timespan(unix_timestamp)? as i128)?
            as f64;
        let exponent = numerator / SECONDS_PER_YEAR / ONE_IN_BASIS_POINTS;
        Some(exponent.exp())
    }

    fn total_scale(&self, decimals: u8, unix_timestamp: i64) -> Option<f64> {
        Some(
            self.pre_update_exp()? * self.post_update_exp(unix_timestamp)?
                / 10_f64.powi(decimals as i32),
        )
    }

    /// Convert a raw amount to its UI representation using the given decimals
    /// field. Excess zeroes or unneeded decimal point are trimmed.
    pub fn amount_to_ui_amount(
        &self,
        amount: U256,
        decimals: u8,
        unix_timestamp: i64,
    ) -> Option<String> {
        let scaled_amount_with_interest =
            amount.as_f64() * self.total_scale(decimals, unix_timestamp)?;
        let ui_amount = format!("{scaled_amount_with_interest:.*}", decimals as usize);
        Some(trim_ui_amount_string(ui_amount, decimals))
    }

    /// Try to convert a UI representation of a token amount to its raw amount
    /// using the given decimals field
    pub fn try_ui_amount_into_amount(
        &self,
        ui_amount: &str,
        decimals: u8,
        unix_timestamp: i64,
    ) -> Result<U256, ProgramError> {
        let scaled_amount = ui_amount
            .parse::<f64>()
            .map_err(|_| ProgramError::InvalidArgument)?;
        let amount = scaled_amount
            / self
                .total_scale(decimals, unix_timestamp)
                .ok_or(ProgramError::InvalidArgument)?;
        if amount > (U256::MAX.as_f64()) || amount < (U256::MIN.as_f64()) || amount.is_nan() {
            Err(ProgramError::InvalidArgument)
        } else {
            // this is important, if you round earlier, you'll get wrong "inf"
            // answers
            Ok(amount.round().as_u256())
        }
    }

    /// The new average rate is the time-weighted average of the current rate
    /// and average rate, solving for r such that:
    ///
    /// ```text
    /// exp(r_1 * t_1) * exp(r_2 * t_2) = exp(r * (t_1 + t_2))
    ///
    /// r_1 * t_1 + r_2 * t_2 = r * (t_1 + t_2)
    ///
    /// r = (r_1 * t_1 + r_2 * t_2) / (t_1 + t_2)
    /// ```
    pub fn time_weighted_average_rate(&self, current_timestamp: i64) -> Option<i16> {
        let initialization_timestamp = i64::from(self.initialization_timestamp) as i128;
        let last_update_timestamp = i64::from(self.last_update_timestamp) as i128;

        let r_1 = i16::from(self.pre_update_average_rate) as i128;
        let t_1 = last_update_timestamp.checked_sub(initialization_timestamp)?;
        let r_2 = i16::from(self.current_rate) as i128;
        let t_2 = (current_timestamp as i128).checked_sub(last_update_timestamp)?;
        let total_timespan = t_1.checked_add(t_2)?;
        let average_rate = if total_timespan == 0 {
            // happens in testing situations, just use the new rate since the earlier
            // one was never practically used
            r_2
        } else {
            r_1.checked_mul(t_1)?
                .checked_add(r_2.checked_mul(t_2)?)?
                .checked_div(total_timespan)?
        };
        average_rate.try_into().ok()
    }
}
impl Extension for InterestBearingConfig {
    const TYPE: ExtensionType = ExtensionType::InterestBearingConfig;
}

#[cfg(test)]
mod tests {
    use {super::*, proptest::prelude::*};

    const INT_SECONDS_PER_YEAR: i64 = 6 * 6 * 24 * 36524;
    const TEST_DECIMALS: u8 = 2;

    #[test]
    fn seconds_per_year() {
        assert_eq!(SECONDS_PER_YEAR, 31_556_736.);
        assert_eq!(INT_SECONDS_PER_YEAR, 31_556_736);
    }

    #[test]
    fn specific_amount_to_ui_amount() {
        const ONE: U256 = U256::new(1_000_000_000_000_000_000_u128);
        // constant 5%
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: 500.into(),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: 500.into(),
        };
        // 1 year at 5% gives a total of exp(0.05) = 1.0512710963760241
        let ui_amount = config
            .amount_to_ui_amount(ONE, 18, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(ui_amount, "1.051271096376024117");
        // with 1 decimal place
        let ui_amount = config
            .amount_to_ui_amount(ONE, 19, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(ui_amount, "0.1051271096376024117");
        // with 10 decimal places
        let ui_amount = config
            .amount_to_ui_amount(ONE, 28, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(ui_amount, "0.0000000001051271096376024175"); // different digits at the end!

        // huge amount with 10 decimal places
        let ui_amount = config
            .amount_to_ui_amount(U256::from(10_000_000_000_u64), 10, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(ui_amount, "1.0512710964");

        // negative
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(-500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(-500),
        };
        // 1 year at -5% gives a total of exp(-0.05) = 0.951229424500714
        let ui_amount = config
            .amount_to_ui_amount(ONE, 18, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(ui_amount, "0.951229424500713905");

        // net out
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(-500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(500),
        };
        // 1 year at -5% and 1 year at 5% gives a total of 1
        let ui_amount = config
            .amount_to_ui_amount(U256::from(1_u64), 0, INT_SECONDS_PER_YEAR * 2)
            .unwrap();
        assert_eq!(ui_amount, "1");

        // huge values
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(500),
        };
        let ui_amount = config
            .amount_to_ui_amount(U256::MAX, 0, INT_SECONDS_PER_YEAR * 2)
            .unwrap();
        assert_eq!(ui_amount, "20386805083448098816");
        let ui_amount = config
            .amount_to_ui_amount(U256::MAX, 0, INT_SECONDS_PER_YEAR * 10_000)
            .unwrap();
        // there's an underflow risk, but it works!
        assert_eq!(ui_amount, "258917064265813826192025834755112557504850551118283225815045099303279643822914042296793377611277551888244755303462190670431480816358154467489350925148558569427069926786360814068189956495940285398273555561779717914539956777398245259214848");
    }

    #[test]
    fn specific_ui_amount_to_amount() {
        // constant 5%
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: 500.into(),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: 500.into(),
        };
        // 1 year at 5% gives a total of exp(0.05) = 1.0512710963760241
        let amount = config
            .try_ui_amount_into_amount("1.0512710963760241", 0, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(1, amount);
        // with 1 decimal place
        let amount = config
            .try_ui_amount_into_amount("0.10512710963760241", 1, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(amount, 1);
        // with 10 decimal places
        let amount = config
            .try_ui_amount_into_amount("0.00000000010512710963760242", 10, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(amount, 1);

        // huge amount with 10 decimal places
        let amount = config
            .try_ui_amount_into_amount("1.0512710963760241", 10, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(amount, 10_000_000_000);

        // negative
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(-500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(-500),
        };
        // 1 year at -5% gives a total of exp(-0.05) = 0.951229424500714
        let amount = config
            .try_ui_amount_into_amount("0.951229424500714", 0, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(amount, 1);

        // net out
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(-500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(500),
        };
        // 1 year at -5% and 1 year at 5% gives a total of 1
        let amount = config
            .try_ui_amount_into_amount("1", 0, INT_SECONDS_PER_YEAR * 2)
            .unwrap();
        assert_eq!(amount, 1);

        // huge values
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: PodI16::from(500),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: PodI16::from(500),
        };
        let amount = config
            .try_ui_amount_into_amount("20386805083448100000", 0, INT_SECONDS_PER_YEAR * 2)
            .unwrap();
        assert_eq!(amount, U256::MAX);
        let amount = config
            .try_ui_amount_into_amount("258917064265813830000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000", 0, INT_SECONDS_PER_YEAR * 10_000)
            .unwrap();
        assert_eq!(amount, U256::MAX);
        // scientific notation "e"
        let amount = config
            .try_ui_amount_into_amount("2.5891706426581383e236", 0, INT_SECONDS_PER_YEAR * 10_000)
            .unwrap();
        assert_eq!(amount, U256::MAX);
        // scientific notation "E"
        let amount = config
            .try_ui_amount_into_amount("2.5891706426581383E236", 0, INT_SECONDS_PER_YEAR * 10_000)
            .unwrap();
        assert_eq!(amount, U256::MAX);

        // overflow u64 fail
        assert_eq!(
            Err(ProgramError::InvalidArgument),
            config.try_ui_amount_into_amount("20386805083448200001", 0, INT_SECONDS_PER_YEAR)
        );

        for fail_ui_amount in ["-0.0000000000000000000001", "inf", "-inf", "NaN"] {
            assert_eq!(
                Err(ProgramError::InvalidArgument),
                config.try_ui_amount_into_amount(fail_ui_amount, 0, INT_SECONDS_PER_YEAR)
            );
        }
    }

    #[test]
    fn specific_amount_to_ui_amount_no_interest() {
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: 0.into(),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: 0.into(),
        };
        for (amount, expected) in [
            (23_u64, "0.23"),
            (110_u64, "1.1"),
            (4200_u64, "42"),
            (0_u64, "0"),
        ] {
            let ui_amount = config
                .amount_to_ui_amount(U256::from(amount), TEST_DECIMALS, INT_SECONDS_PER_YEAR)
                .unwrap();
            assert_eq!(ui_amount, expected);
        }
    }

    #[test]
    fn specific_ui_amount_to_amount_no_interest() {
        let config = InterestBearingConfig {
            rate_authority: OptionalNonZeroPubkey::default(),
            initialization_timestamp: 0.into(),
            pre_update_average_rate: 0.into(),
            last_update_timestamp: INT_SECONDS_PER_YEAR.into(),
            current_rate: 0.into(),
        };
        for (ui_amount, expected) in [
            ("0.23", 23),
            ("0.20", 20),
            ("0.2000", 20),
            (".2", 20),
            ("1.1", 110),
            ("1.10", 110),
            ("42", 4200),
            ("42.", 4200),
            ("0", 0),
        ] {
            let amount = config
                .try_ui_amount_into_amount(ui_amount, TEST_DECIMALS, INT_SECONDS_PER_YEAR)
                .unwrap();
            assert_eq!(expected, amount);
        }

        // this is invalid with normal mints, but rounding for this mint makes it ok
        let amount = config
            .try_ui_amount_into_amount("0.111", TEST_DECIMALS, INT_SECONDS_PER_YEAR)
            .unwrap();
        assert_eq!(11, amount);

        // fail if invalid ui_amount passed in
        for ui_amount in ["", ".", "0.t"] {
            assert_eq!(
                Err(ProgramError::InvalidArgument),
                config.try_ui_amount_into_amount(ui_amount, TEST_DECIMALS, INT_SECONDS_PER_YEAR),
            );
        }
    }

    prop_compose! {
        /// Three values in ascending order
        fn low_middle_high()
            (middle in 1..i64::MAX - 1)
            (low in 0..=middle, middle in Just(middle), high in middle..=i64::MAX)
                        -> (i64, i64, i64) {
           (low, middle, high)
       }
    }

    proptest! {
        #[test]
        fn time_weighted_average_calc(
            current_rate in i16::MIN..i16::MAX,
            pre_update_average_rate in i16::MIN..i16::MAX,
            (initialization_timestamp, last_update_timestamp, current_timestamp) in low_middle_high(),
        ) {
            let config = InterestBearingConfig {
                rate_authority: OptionalNonZeroPubkey::default(),
                initialization_timestamp: initialization_timestamp.into(),
                pre_update_average_rate: pre_update_average_rate.into(),
                last_update_timestamp: last_update_timestamp.into(),
                current_rate: current_rate.into(),
            };
            let new_rate = config.time_weighted_average_rate(current_timestamp).unwrap();
            if pre_update_average_rate <= current_rate {
                assert!(pre_update_average_rate <= new_rate);
                assert!(new_rate <= current_rate);
            } else {
                assert!(current_rate <= new_rate);
                assert!(new_rate <= pre_update_average_rate);
            }
        }

        #[test]
        fn amount_to_ui_amount(
            current_rate in i16::MIN..i16::MAX,
            pre_update_average_rate in i16::MIN..i16::MAX,
            (initialization_timestamp, last_update_timestamp, current_timestamp) in low_middle_high(),
            amount in 0..=u64::MAX, // TODO: change to U256::MAX
            decimals in 0u8..20u8,
        ) {
            let config = InterestBearingConfig {
                rate_authority: OptionalNonZeroPubkey::default(),
                initialization_timestamp: initialization_timestamp.into(),
                pre_update_average_rate: pre_update_average_rate.into(),
                last_update_timestamp: last_update_timestamp.into(),
                current_rate: current_rate.into(),
            };
            let ui_amount = config.amount_to_ui_amount(U256::from(amount), decimals, current_timestamp);
            assert!(ui_amount.is_some());
        }
    }
}
