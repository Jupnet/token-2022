#[cfg(feature = "serde-traits")]
use {
    crate::serialization::coption_fromstr,
    serde::{Deserialize, Serialize},
};
use {
    crate::{check_program_account, error::TokenError, instruction::TokenInstruction},
    ethnum::U256,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program_error::ProgramError,
        program_option::COption,
        pubkey::Pubkey,
    },
    std::convert::TryFrom,
};

/// Transfer Fee extension instructions
#[cfg_attr(feature = "serde-traits", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "serde-traits",
    serde(rename_all = "camelCase", rename_all_fields = "camelCase")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum TransferFeeInstruction {
    /// Initialize the transfer fee on a new mint.
    ///
    /// Fails if the mint has already been initialized, so must be called before
    /// `InitializeMint`.
    ///
    /// The mint must have exactly enough space allocated for the base mint (82
    /// bytes), plus 83 bytes of padding, 1 byte reserved for the account type,
    /// then space required for this extension, plus any others.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` The mint to initialize.
    InitializeTransferFeeConfig {
        /// Pubkey that may update the fees
        #[cfg_attr(feature = "serde-traits", serde(with = "coption_fromstr"))]
        transfer_fee_config_authority: COption<Pubkey>,
        /// Withdraw instructions must be signed by this key
        #[cfg_attr(feature = "serde-traits", serde(with = "coption_fromstr"))]
        withdraw_withheld_authority: COption<Pubkey>,
        /// Amount of transfer collected as fees, expressed as basis points of
        /// the transfer amount
        transfer_fee_basis_points: u16,
        /// Maximum fee assessed on transfers
        maximum_fee: U256,
    },
    /// Transfer, providing expected mint information and fees
    ///
    /// This instruction succeeds if the mint has no configured transfer fee
    /// and the provided fee is 0. This allows applications to use
    /// `TransferCheckedWithFee` with any mint.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   * Single owner/delegate
    ///   0. `[writable]` The source account. May include the
    ///      `TransferFeeAmount` extension.
    ///   1. `[]` The token mint. May include the `TransferFeeConfig` extension.
    ///   2. `[writable]` The destination account. May include the
    ///      `TransferFeeAmount` extension.
    ///   3. `[signer]` The source account's owner/delegate.
    ///
    ///   * Multisignature owner/delegate
    ///   0. `[writable]` The source account.
    ///   1. `[]` The token mint.
    ///   2. `[writable]` The destination account.
    ///   3. `[]` The source account's multisignature owner/delegate.
    ///   4. `..4+M` `[signer]` M signer accounts.
    TransferCheckedWithFee {
        /// The amount of tokens to transfer.
        amount: U256,
        /// Expected number of base 10 digits to the right of the decimal place.
        decimals: u8,
        /// Expected fee assessed on this transfer, calculated off-chain based
        /// on the `transfer_fee_basis_points` and `maximum_fee` of the mint.
        /// May be 0 for a mint without a configured transfer fee.
        fee: U256,
    },
    /// Transfer all withheld tokens in the mint to an account. Signed by the
    /// mint's withdraw withheld tokens authority.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   * Single owner/delegate
    ///   0. `[writable]` The token mint. Must include the `TransferFeeConfig`
    ///      extension.
    ///   1. `[writable]` The fee receiver account. Must include the
    ///      `TransferFeeAmount` extension associated with the provided mint.
    ///   2. `[signer]` The mint's `withdraw_withheld_authority`.
    ///
    ///   * Multisignature owner/delegate
    ///   0. `[writable]` The token mint.
    ///   1. `[writable]` The destination account.
    ///   2. `[]` The mint's multisig `withdraw_withheld_authority`.
    ///   3. `..3+M `[signer]` M signer accounts.
    WithdrawWithheldTokensFromMint,
    /// Transfer all withheld tokens to an account. Signed by the mint's
    /// withdraw withheld tokens authority.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   * Single owner/delegate
    ///   0. `[]` The token mint. Must include the `TransferFeeConfig`
    ///      extension.
    ///   1. `[writable]` The fee receiver account. Must include the
    ///      `TransferFeeAmount` extension and be associated with the provided
    ///      mint.
    ///   2. `[signer]` The mint's `withdraw_withheld_authority`.
    ///   3. `..3+N` `[writable]` The source accounts to withdraw from.
    ///
    ///   * Multisignature owner/delegate
    ///   0. `[]` The token mint.
    ///   1. `[writable]` The destination account.
    ///   2. `[]` The mint's multisig `withdraw_withheld_authority`.
    ///   3. `..3+M` `[signer]` M signer accounts.
    ///   4. `3+M+1..3+M+N` `[writable]` The source accounts to withdraw from.
    WithdrawWithheldTokensFromAccounts {
        /// Number of token accounts harvested
        num_token_accounts: u8,
    },
    /// Permissionless instruction to transfer all withheld tokens to the mint.
    ///
    /// Succeeds for frozen accounts.
    ///
    /// Accounts provided should include the `TransferFeeAmount` extension. If
    /// not, the account is skipped.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` The mint.
    ///   1. `..1+N` `[writable]` The source accounts to harvest from.
    HarvestWithheldTokensToMint,
    /// Set transfer fee. Only supported for mints that include the
    /// `TransferFeeConfig` extension.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   * Single authority
    ///   0. `[writable]` The mint.
    ///   1. `[signer]` The mint's fee account owner.
    ///
    ///   * Multisignature authority
    ///   0. `[writable]` The mint.
    ///   1. `[]` The mint's multisignature fee account owner.
    ///   2. `..2+M` `[signer]` M signer accounts.
    SetTransferFee {
        /// Amount of transfer collected as fees, expressed as basis points of
        /// the transfer amount
        transfer_fee_basis_points: u16,
        /// Maximum fee assessed on transfers
        maximum_fee: U256,
    },
}
impl TransferFeeInstruction {
    /// Unpacks a byte buffer into a `TransferFeeInstruction`
    pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
        use TokenError::InvalidInstruction;

        let (&tag, rest) = input.split_first().ok_or(InvalidInstruction)?;
        Ok(match tag {
            0 => {
                let (transfer_fee_config_authority, rest) =
                    TokenInstruction::unpack_pubkey_option(rest)?;
                let (withdraw_withheld_authority, rest) =
                    TokenInstruction::unpack_pubkey_option(rest)?;
                let (transfer_fee_basis_points, rest) = TokenInstruction::unpack_u16(rest)?;
                let (maximum_fee, _) = TokenInstruction::unpack_u256(rest)?;
                Self::InitializeTransferFeeConfig {
                    transfer_fee_config_authority,
                    withdraw_withheld_authority,
                    transfer_fee_basis_points,
                    maximum_fee,
                }
            }
            1 => {
                let (amount, decimals, rest) = TokenInstruction::unpack_amount_decimals_u256(rest)?;
                let (fee, _) = TokenInstruction::unpack_u256(rest)?;
                Self::TransferCheckedWithFee {
                    amount,
                    decimals,
                    fee,
                }
            }
            2 => Self::WithdrawWithheldTokensFromMint,
            3 => {
                let (&num_token_accounts, _) = rest.split_first().ok_or(InvalidInstruction)?;
                Self::WithdrawWithheldTokensFromAccounts { num_token_accounts }
            }
            4 => Self::HarvestWithheldTokensToMint,
            5 => {
                let (transfer_fee_basis_points, rest) = TokenInstruction::unpack_u16(rest)?;
                let (maximum_fee, _) = TokenInstruction::unpack_u256(rest)?;
                Self::SetTransferFee {
                    transfer_fee_basis_points,
                    maximum_fee,
                }
            }
            _ => return Err(TokenError::InvalidInstruction.into()),
        })
    }

    /// Packs a `TransferFeeInstruction` into a byte buffer.
    pub fn pack(&self, buffer: &mut Vec<u8>) {
        match *self {
            Self::InitializeTransferFeeConfig {
                ref transfer_fee_config_authority,
                ref withdraw_withheld_authority,
                transfer_fee_basis_points,
                maximum_fee,
            } => {
                buffer.push(0);
                TokenInstruction::pack_pubkey_option(transfer_fee_config_authority, buffer);
                TokenInstruction::pack_pubkey_option(withdraw_withheld_authority, buffer);
                buffer.extend_from_slice(&transfer_fee_basis_points.to_le_bytes());
                buffer.extend_from_slice(&maximum_fee.to_le_bytes());
            }
            Self::TransferCheckedWithFee {
                amount,
                decimals,
                fee,
            } => {
                buffer.push(1);
                buffer.extend_from_slice(&amount.to_le_bytes());
                buffer.extend_from_slice(&decimals.to_le_bytes());
                buffer.extend_from_slice(&fee.to_le_bytes());
            }
            Self::WithdrawWithheldTokensFromMint => {
                buffer.push(2);
            }
            Self::WithdrawWithheldTokensFromAccounts { num_token_accounts } => {
                buffer.push(3);
                buffer.push(num_token_accounts);
            }
            Self::HarvestWithheldTokensToMint => {
                buffer.push(4);
            }
            Self::SetTransferFee {
                transfer_fee_basis_points,
                maximum_fee,
            } => {
                buffer.push(5);
                buffer.extend_from_slice(&transfer_fee_basis_points.to_le_bytes());
                buffer.extend_from_slice(&maximum_fee.to_le_bytes());
            }
        }
    }
}

fn encode_instruction_data(transfer_fee_instruction: TransferFeeInstruction) -> Vec<u8> {
    let mut data = TokenInstruction::TransferFeeExtension.pack();
    transfer_fee_instruction.pack(&mut data);
    data
}

/// Create a `InitializeTransferFeeConfig` instruction
pub fn initialize_transfer_fee_config(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    transfer_fee_config_authority: Option<&Pubkey>,
    withdraw_withheld_authority: Option<&Pubkey>,
    transfer_fee_basis_points: u16,
    maximum_fee: U256,
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let transfer_fee_config_authority = transfer_fee_config_authority.cloned().into();
    let withdraw_withheld_authority = withdraw_withheld_authority.cloned().into();
    let data = encode_instruction_data(TransferFeeInstruction::InitializeTransferFeeConfig {
        transfer_fee_config_authority,
        withdraw_withheld_authority,
        transfer_fee_basis_points,
        maximum_fee,
    });

    Ok(Instruction {
        program_id: *token_program_id,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    })
}

/// Create a `TransferCheckedWithFee` instruction
#[allow(clippy::too_many_arguments)]
pub fn transfer_checked_with_fee(
    token_program_id: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    signers: &[&Pubkey],
    amount: U256,
    decimals: u8,
    fee: U256,
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let data = encode_instruction_data(TransferFeeInstruction::TransferCheckedWithFee {
        amount,
        decimals,
        fee,
    });

    let mut accounts = Vec::with_capacity(4 + signers.len());
    accounts.push(AccountMeta::new(*source, false));
    accounts.push(AccountMeta::new_readonly(*mint, false));
    accounts.push(AccountMeta::new(*destination, false));
    accounts.push(AccountMeta::new_readonly(*authority, signers.is_empty()));
    for signer in signers.iter() {
        accounts.push(AccountMeta::new_readonly(**signer, true));
    }

    Ok(Instruction {
        program_id: *token_program_id,
        accounts,
        data,
    })
}

/// Creates a `WithdrawWithheldTokensFromMint` instruction
pub fn withdraw_withheld_tokens_from_mint(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    signers: &[&Pubkey],
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let mut accounts = Vec::with_capacity(3 + signers.len());
    accounts.push(AccountMeta::new(*mint, false));
    accounts.push(AccountMeta::new(*destination, false));
    accounts.push(AccountMeta::new_readonly(*authority, signers.is_empty()));
    for signer in signers.iter() {
        accounts.push(AccountMeta::new_readonly(**signer, true));
    }

    Ok(Instruction {
        program_id: *token_program_id,
        accounts,
        data: encode_instruction_data(TransferFeeInstruction::WithdrawWithheldTokensFromMint),
    })
}

/// Creates a `WithdrawWithheldTokensFromAccounts` instruction
pub fn withdraw_withheld_tokens_from_accounts(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    signers: &[&Pubkey],
    sources: &[&Pubkey],
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let num_token_accounts =
        u8::try_from(sources.len()).map_err(|_| ProgramError::InvalidInstructionData)?;
    let mut accounts = Vec::with_capacity(3 + signers.len() + sources.len());
    accounts.push(AccountMeta::new_readonly(*mint, false));
    accounts.push(AccountMeta::new(*destination, false));
    accounts.push(AccountMeta::new_readonly(*authority, signers.is_empty()));
    for signer in signers.iter() {
        accounts.push(AccountMeta::new_readonly(**signer, true));
    }
    for source in sources.iter() {
        accounts.push(AccountMeta::new(**source, false));
    }

    Ok(Instruction {
        program_id: *token_program_id,
        accounts,
        data: encode_instruction_data(TransferFeeInstruction::WithdrawWithheldTokensFromAccounts {
            num_token_accounts,
        }),
    })
}

/// Creates a `HarvestWithheldTokensToMint` instruction
pub fn harvest_withheld_tokens_to_mint(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    sources: &[&Pubkey],
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let mut accounts = Vec::with_capacity(1 + sources.len());
    accounts.push(AccountMeta::new(*mint, false));
    for source in sources.iter() {
        accounts.push(AccountMeta::new(**source, false));
    }
    Ok(Instruction {
        program_id: *token_program_id,
        accounts,
        data: encode_instruction_data(TransferFeeInstruction::HarvestWithheldTokensToMint),
    })
}

/// Creates a `SetTransferFee` instruction
pub fn set_transfer_fee(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    authority: &Pubkey,
    signers: &[&Pubkey],
    transfer_fee_basis_points: u16,
    maximum_fee: U256,
) -> Result<Instruction, ProgramError> {
    check_program_account(token_program_id)?;
    let mut accounts = Vec::with_capacity(2 + signers.len());
    accounts.push(AccountMeta::new(*mint, false));
    accounts.push(AccountMeta::new_readonly(*authority, signers.is_empty()));
    for signer in signers.iter() {
        accounts.push(AccountMeta::new_readonly(**signer, true));
    }

    Ok(Instruction {
        program_id: *token_program_id,
        accounts,
        data: encode_instruction_data(TransferFeeInstruction::SetTransferFee {
            transfer_fee_basis_points,
            maximum_fee,
        }),
    })
}

#[cfg(test)]
mod test {
    use ethnum::AsU256;

    use super::*;

    #[test]
    fn test_instruction_packing() {
        let check = TransferFeeInstruction::InitializeTransferFeeConfig {
            transfer_fee_config_authority: COption::Some(Pubkey::new_from_array([11u8; 32])),
            withdraw_withheld_authority: COption::None,
            transfer_fee_basis_points: 111,
            maximum_fee: U256::MAX,
        };
        let mut packed = vec![];
        check.pack(&mut packed);
        let mut expect = vec![0, 1];
        expect.extend_from_slice(&[11u8; 32]);
        expect.extend_from_slice(&[0]);
        expect.extend_from_slice(&111u16.to_le_bytes());
        expect.extend_from_slice(&U256::MAX.to_le_bytes());
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);

        let check = TransferFeeInstruction::TransferCheckedWithFee {
            amount: U256::from(24_u64),
            decimals: 24,
            fee: U256::from(23_u64),
        };
        let mut packed = vec![];
        check.pack(&mut packed);
        let mut expect = vec![1];
        expect.extend_from_slice(&24.as_u256().to_le_bytes());
        expect.extend_from_slice(&[24u8]);
        expect.extend_from_slice(&23.as_u256().to_le_bytes());
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);

        let check = TransferFeeInstruction::WithdrawWithheldTokensFromMint;
        let mut packed = vec![];
        check.pack(&mut packed);
        let expect = [2];
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);

        let num_token_accounts = 255;
        let check =
            TransferFeeInstruction::WithdrawWithheldTokensFromAccounts { num_token_accounts };
        let mut packed = vec![];
        check.pack(&mut packed);
        let expect = [3, num_token_accounts];
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);

        let check = TransferFeeInstruction::HarvestWithheldTokensToMint;
        let mut packed = vec![];
        check.pack(&mut packed);
        let expect = [4];
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);

        let check = TransferFeeInstruction::SetTransferFee {
            transfer_fee_basis_points: u16::MAX,
            maximum_fee: U256::MAX,
        };
        let mut packed = vec![];
        check.pack(&mut packed);
        let mut expect = vec![5];
        expect.extend_from_slice(&u16::MAX.to_le_bytes());
        expect.extend_from_slice(&U256::MAX.to_le_bytes());
        assert_eq!(packed, expect);
        let unpacked = TransferFeeInstruction::unpack(&expect).unwrap();
        assert_eq!(unpacked, check);
    }
}
