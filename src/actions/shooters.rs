use std::{boxed::Box, sync::Arc};

use color_eyre::eyre::OptionExt;
use goose::{
    config::GooseConfiguration,
    goose::{Scenario, Transaction, TransactionFunction},
    metrics::GooseMetrics,
    GooseAttack,
};
use starknet::{
    core::types::{Call, Felt, InvokeTransactionResult},
    providers::jsonrpc::JsonRpcMethod,
};

use crate::{
    actions::goose::{send_execution, GooseWriteUserState},
    config::GatlingConfig,
};

use super::{
    goose::{
        goose_write_user_wait_last_tx, make_goose_config, setup, verify_transactions,
        TransactionBlocks,
    },
    setup::{GatlingSetup, StarknetAccount},
};

pub mod mint;
pub mod transfer;

pub struct ShooterAttack {
    pub goose_metrics: GooseMetrics,
    pub first_block: u64,
    pub last_block: u64,
}

pub trait Shooter {
    const NAME: &'static str;

    async fn setup(setup: &mut GatlingSetup) -> color_eyre::Result<Self>
    where
        Self: Sized;

    fn get_goose_config(
        config: &GatlingConfig,
        amount: u64,
    ) -> color_eyre::Result<GooseConfiguration> {
        make_goose_config(config, amount, Self::NAME)
    }

    async fn goose_attack(
        self: Arc<Self>,
        config: GooseConfiguration,
        accounts: Vec<StarknetAccount>,
    ) -> color_eyre::Result<ShooterAttack>
    where
        Self: Send + Sync + 'static,
    {
        let setup: TransactionFunction = setup(accounts, config.iterations).await?;

        let submission: TransactionFunction = Self::execute(self.clone());

        let finalizing: TransactionFunction = goose_write_user_wait_last_tx();

        let blocks: Arc<TransactionBlocks> = Arc::default();
        let blocks_cloned = blocks.clone();

        let verify_transactions = Transaction::new(Arc::new(move |user| {
            Box::pin(verify_transactions(user, blocks_cloned.clone()))
        }));

        let goose_attack = GooseAttack::initialize_with_config(config)?.register_scenario(
            Scenario::new(Self::NAME)
                .register_transaction(Transaction::new(setup).set_name("Setup").set_on_start())
                .register_transaction(
                    Transaction::new(submission)
                        .set_name("Transaction Submission")
                        .set_sequence(1),
                )
                .register_transaction(
                    Transaction::new(finalizing)
                        .set_name("Finalizing")
                        .set_sequence(2)
                        .set_on_stop(),
                )
                .register_transaction(
                    verify_transactions
                        .set_name("Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        );

        let metrics = goose_attack.execute().await?;

        let blocks = Arc::into_inner(blocks).ok_or_eyre(
            "Transaction blocks arc has multiple references after goose verification",
        )?;

        Ok(ShooterAttack {
            goose_metrics: metrics,
            first_block: blocks.first.into_inner(),
            last_block: blocks.last.into_inner(),
        })
    }

    fn execute(self: Arc<Self>) -> TransactionFunction
    where
        Self: Send + Sync + 'static,
    {
        Arc::new(move |user| {
            let shooter = self.clone();

            Box::pin(async move {
                let GooseWriteUserState { account, nonce, .. } = user
                    .get_session_data::<GooseWriteUserState>()
                    .expect("Should be in a goose user with GooseUserState session data");

                let call = shooter.get_execution_data(account);

                let response: InvokeTransactionResult = send_execution(
                    user,
                    vec![call],
                    *nonce,
                    &account.clone(),
                    JsonRpcMethod::AddInvokeTransaction,
                )
                .await?
                .0;

                let GooseWriteUserState { nonce, prev_tx, .. } =
                    user.get_session_data_mut::<GooseWriteUserState>().expect(
                        "Should be successful as we already asserted that the session data is a GooseUserState",
                    );

                *nonce += Felt::ONE;

                prev_tx.push(response.transaction_hash);

                Ok(())
            })
        })
    }

    fn get_execution_data(&self, account: &StarknetAccount) -> Call;
}
