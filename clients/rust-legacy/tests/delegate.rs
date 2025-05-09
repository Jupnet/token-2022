mod program_test;
use {
    ethnum::U256,
    program_test::{TestContext, TokenContext},
    solana_program_test::tokio,
    solana_sdk::{
        instruction::InstructionError, pubkey::Pubkey, signature::Signer, signer::keypair::Keypair,
        transaction::TransactionError, transport::TransportError,
    },
    spl_token_2022::error::TokenError,
    spl_token_client::token::{ExtensionInitializationParams, TokenError as TokenClientError},
};

#[derive(PartialEq)]
enum TransferMode {
    All,
    CheckedOnly,
}

#[derive(PartialEq)]
enum ApproveMode {
    Unchecked,
    Checked,
}

#[derive(PartialEq)]
enum OwnerMode {
    SelfOwned,
    External,
}

async fn run_basic(
    context: TestContext,
    owner_mode: OwnerMode,
    transfer_mode: TransferMode,
    approve_mode: ApproveMode,
) {
    let TokenContext {
        mint_authority,
        token,
        token_unchecked,
        alice,
        bob,
        ..
    } = context.token_context.unwrap();

    let alice_account = match owner_mode {
        OwnerMode::SelfOwned => {
            token
                .create_auxiliary_token_account(&alice, &alice.pubkey())
                .await
                .unwrap();
            alice.pubkey()
        }
        OwnerMode::External => {
            let alice_account = Keypair::new();
            token
                .create_auxiliary_token_account(&alice_account, &alice.pubkey())
                .await
                .unwrap();
            alice_account.pubkey()
        }
    };
    let bob_account = Keypair::new();
    token
        .create_auxiliary_token_account(&bob_account, &bob.pubkey())
        .await
        .unwrap();
    let bob_account = bob_account.pubkey();

    // mint tokens
    let amount = U256::from(100_u64);
    token
        .mint_to(
            &alice_account,
            &mint_authority.pubkey(),
            amount,
            &[&mint_authority],
        )
        .await
        .unwrap();

    // delegate to bob
    let delegated_amount = U256::from(10_u64);
    match approve_mode {
        ApproveMode::Unchecked => token_unchecked
            .approve(
                &alice_account,
                &bob.pubkey(),
                &alice.pubkey(),
                delegated_amount,
                &[&alice],
            )
            .await
            .unwrap(),
        ApproveMode::Checked => token
            .approve(
                &alice_account,
                &bob.pubkey(),
                &alice.pubkey(),
                delegated_amount,
                &[&alice],
            )
            .await
            .unwrap(),
    }

    // transfer too much is not ok
    let error = token
        .transfer(
            &alice_account,
            &bob_account,
            &bob.pubkey(),
            delegated_amount.checked_add(U256::ONE).unwrap(),
            &[&bob],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        TokenClientError::Client(Box::new(TransportError::TransactionError(
            TransactionError::InstructionError(
                0,
                InstructionError::Custom(TokenError::InsufficientFunds as u32)
            )
        )))
    );

    // transfer is ok
    if transfer_mode == TransferMode::All {
        token_unchecked
            .transfer(
                &alice_account,
                &bob_account,
                &bob.pubkey(),
                U256::ONE,
                &[&bob],
            )
            .await
            .unwrap();
    }

    token
        .transfer(
            &alice_account,
            &bob_account,
            &bob.pubkey(),
            U256::ONE,
            &[&bob],
        )
        .await
        .unwrap();

    // burn is ok
    token_unchecked
        .burn(&alice_account, &bob.pubkey(), U256::ONE, &[&bob])
        .await
        .unwrap();
    token
        .burn(&alice_account, &bob.pubkey(), U256::ONE, &[&bob])
        .await
        .unwrap();

    // wrong signer
    let keypair = &Keypair::new();
    let error = token
        .transfer(
            &alice_account,
            &bob_account,
            &keypair.pubkey(),
            U256::ONE,
            &[keypair],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        TokenClientError::Client(Box::new(TransportError::TransactionError(
            TransactionError::InstructionError(
                0,
                InstructionError::Custom(TokenError::OwnerMismatch as u32)
            )
        )))
    );

    // revoke
    token
        .revoke(&alice_account, &alice.pubkey(), &[&alice])
        .await
        .unwrap();

    // now fails
    let error = token
        .transfer(
            &alice_account,
            &bob_account,
            &bob.pubkey(),
            U256::from(2_u64),
            &[&bob],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        TokenClientError::Client(Box::new(TransportError::TransactionError(
            TransactionError::InstructionError(
                0,
                InstructionError::Custom(TokenError::OwnerMismatch as u32)
            )
        )))
    );
}

#[tokio::test]
async fn basic() {
    let mut context = TestContext::new().await;
    context.init_token_with_mint(vec![]).await.unwrap();
    run_basic(
        context,
        OwnerMode::External,
        TransferMode::All,
        ApproveMode::Unchecked,
    )
    .await;
}

#[tokio::test]
async fn basic_checked() {
    let mut context = TestContext::new().await;
    context.init_token_with_mint(vec![]).await.unwrap();
    run_basic(
        context,
        OwnerMode::External,
        TransferMode::All,
        ApproveMode::Checked,
    )
    .await;
}

#[tokio::test]
async fn basic_self_owned() {
    let mut context = TestContext::new().await;
    context.init_token_with_mint(vec![]).await.unwrap();
    run_basic(
        context,
        OwnerMode::SelfOwned,
        TransferMode::All,
        ApproveMode::Checked,
    )
    .await;
}

#[tokio::test]
async fn basic_with_extension() {
    let mut context = TestContext::new().await;
    context
        .init_token_with_mint(vec![ExtensionInitializationParams::TransferFeeConfig {
            transfer_fee_config_authority: Some(Pubkey::new_unique()),
            withdraw_withheld_authority: Some(Pubkey::new_unique()),
            transfer_fee_basis_points: 100u16,
            maximum_fee: U256::from(1_000_u64),
        }])
        .await
        .unwrap();
    run_basic(
        context,
        OwnerMode::External,
        TransferMode::CheckedOnly,
        ApproveMode::Unchecked,
    )
    .await;
}

#[tokio::test]
async fn basic_with_extension_checked() {
    let mut context = TestContext::new().await;
    context
        .init_token_with_mint(vec![ExtensionInitializationParams::TransferFeeConfig {
            transfer_fee_config_authority: Some(Pubkey::new_unique()),
            withdraw_withheld_authority: Some(Pubkey::new_unique()),
            transfer_fee_basis_points: 100u16,
            maximum_fee: U256::from(1_000_u64),
        }])
        .await
        .unwrap();
    run_basic(
        context,
        OwnerMode::External,
        TransferMode::CheckedOnly,
        ApproveMode::Checked,
    )
    .await;
}

#[tokio::test]
async fn basic_self_owned_with_extension() {
    let mut context = TestContext::new().await;
    context
        .init_token_with_mint(vec![ExtensionInitializationParams::TransferFeeConfig {
            transfer_fee_config_authority: Some(Pubkey::new_unique()),
            withdraw_withheld_authority: Some(Pubkey::new_unique()),
            transfer_fee_basis_points: 100u16,
            maximum_fee: U256::from(1_000_u64),
        }])
        .await
        .unwrap();
    run_basic(
        context,
        OwnerMode::SelfOwned,
        TransferMode::CheckedOnly,
        ApproveMode::Checked,
    )
    .await;
}
