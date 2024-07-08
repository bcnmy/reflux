use std::collections::HashMap;
use std::future;

use futures::StreamExt;
use log::{error, info};
use ruint::__private::ruint_macro::uint;
use ruint::aliases::U256;
use ruint::Uint;
use thiserror::Error;

use config::Config;

use crate::BridgeResult;
use crate::erc20_utils::ERC20Utils;
use crate::source::{EthereumTransaction, RequiredApprovalDetails, RouteSource};
use crate::token_price::TokenPriceProvider;
use crate::token_price::utils::{Errors, get_token_amount_from_value_in_usd};

pub struct SettlementEngine<
    'config,
    Source: RouteSource,
    PriceProvider: TokenPriceProvider,
    ERC20TokenUtils: ERC20Utils,
> {
    source: Source,
    config: &'config Config,
    price_provider: PriceProvider,
    erc20_util: ERC20TokenUtils,
}

#[derive(Debug)]
pub enum TransactionType {
    Approval,
    Bridge,
}

#[derive(Debug)]
pub struct TransactionWithType {
    transaction: EthereumTransaction,
    transaction_type: TransactionType,
}

const GENERATE_TRANSACTIONS_CONCURRENCY: usize = 10;

impl<
        'config,
        Source: RouteSource,
        PriceProvider: TokenPriceProvider,
        ERC20TokenUtils: ERC20Utils,
    > SettlementEngine<'config, Source, PriceProvider, ERC20TokenUtils>
{
    pub fn new(
        config: &'config Config,
        source: Source,
        price_provider: PriceProvider,
        erc20_util: ERC20TokenUtils,
    ) -> Self {
        SettlementEngine { source, config, price_provider, erc20_util }
    }

    async fn generate_transactions(
        &self,
        routes: &Vec<BridgeResult<'config>>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        let (results, failed): (
            Vec<
                Result<
                    (Vec<EthereumTransaction>, Vec<RequiredApprovalDetails>),
                    SettlementEngineErrors<Source, PriceProvider>,
                >,
            >,
            _,
        ) = futures::stream::iter(routes.into_iter())
            .map(|route| async move {
                info!("Generating transactions for route: {:?}", route.route);

                let token_amount = get_token_amount_from_value_in_usd(
                    self.config,
                    &self.price_provider,
                    &route.route.from_token.symbol,
                    route.route.from_chain.id,
                    &route.source_amount_in_usd,
                )
                .await
                .map_err(|err| SettlementEngineErrors::GetTokenAmountFromValueInUsdError(err))?;

                info!("Token amount: {:?} for route {:?}", token_amount, route);

                let (ethereum_transactions, required_approval_details) = self
                    .source
                    .generate_route_transactions(
                        &route.route,
                        &token_amount,
                        &route.from_address,
                        &route.to_address,
                    )
                    .await
                    .map_err(|err| SettlementEngineErrors::GenerateTransactionsError(err))?;

                info!("Generated transactions: {:?} for route {:?}", ethereum_transactions, route);

                Ok::<_, SettlementEngineErrors<Source, PriceProvider>>((
                    ethereum_transactions,
                    required_approval_details,
                ))
            })
            .buffer_unordered(GENERATE_TRANSACTIONS_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .partition(Result::is_ok);

        let failed: Vec<_> = failed.into_iter().map(Result::unwrap_err).collect();
        if !failed.is_empty() {
            error!("Failed to generate transactions: {:?}", failed);
        }

        if results.is_empty() {
            error!("No transactions generated");
            return Err(SettlementEngineErrors::NoTransactionsGenerated);
        }

        let (ethereum_transactions, required_approval_details): (Vec<Vec<_>>, Vec<Vec<_>>) =
            results.into_iter().map(Result::unwrap).unzip();

        let mut ethereum_transactions: Vec<_> = ethereum_transactions
            .into_iter()
            .flatten()
            .map(|t| TransactionWithType {
                transaction: t,
                transaction_type: TransactionType::Bridge,
            })
            .collect();

        let required_approval_details: Vec<_> =
            required_approval_details.into_iter().flatten().collect();
        let required_approval_transactions =
            self.generate_transactions_for_approvals(&required_approval_details).await?;

        info!("Generated transactions: {:?}", ethereum_transactions);
        info!("Required approvals: {:?}", required_approval_details);

        ethereum_transactions.extend(required_approval_transactions);

        info!("Final Transactions: {:?}", ethereum_transactions);

        Ok(ethereum_transactions)
    }

    async fn generate_transactions_for_approvals(
        &self,
        approvals: &Vec<RequiredApprovalDetails>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        // Group the approvals and combine them based on chain_id, token_address, spender and target
        let mut approvals_grouped =
            HashMap::<(u32, &String, &String, &String), Vec<&RequiredApprovalDetails>>::new();
        for approval in approvals {
            let key =
                (approval.chain_id, &approval.token_address, &approval.spender, &approval.target);
            let arr = approvals_grouped.get_mut(&key);
            if arr.is_none() {
                approvals_grouped.insert(key, vec![&approval]);
            } else {
                arr.unwrap().push(&approval);
            }
        }

        // Merge the approvals with the same key
        let merged_approvals: Vec<RequiredApprovalDetails> = approvals_grouped
            .into_iter()
            .map(|(_, approvals)| {
                let mut amount =
                    approvals.iter().map(|approval| approval.amount).reduce(|a, b| (a + b));

                if amount.is_none() {
                    error!(
                        "Failed to merge approvals due to error in amount reduction: {:?}",
                        approvals
                    );

                    // Set 0 approval if there's an error
                    amount = Some(Uint::ZERO);
                }

                let amount = amount.unwrap();

                RequiredApprovalDetails {
                    chain_id: approvals[0].chain_id,
                    token_address: approvals[0].token_address.clone(),
                    spender: approvals[0].spender.clone(),
                    target: approvals[0].target.clone(),
                    amount,
                }
            })
            .collect();

        // Generate Transactions for the merged approvals
        let (approval_transactions, failed): (Vec<_>, _) =
            futures::stream::iter(merged_approvals.into_iter())
                .map(|approval| async move {
                    Ok::<_, ERC20TokenUtils::Error>(
                        self.erc20_util.generate_required_approval_transaction(&approval).await?,
                    )
                })
                .buffer_unordered(GENERATE_TRANSACTIONS_CONCURRENCY)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .partition(Result::is_ok);

        if !failed.is_empty() {
            error!("Failed to generate approval transactions: {:?}", failed);
        }

        if approval_transactions.is_empty() {
            error!("No approval transactions approved");
            return Err(SettlementEngineErrors::NoTransactionsApproved);
        }

        Ok(approval_transactions.into_iter().map(Result::unwrap).collect())
    }
}

#[derive(Error, Debug)]
pub enum SettlementEngineErrors<Source: RouteSource, PriceProvider: TokenPriceProvider> {
    #[error("Error generating transactions: {0}")]
    GenerateTransactionsError(Source::GenerateRouteTransactionsError),

    #[error("Error getting token amount from value in USD: {0}")]
    GetTokenAmountFromValueInUsdError(Errors<PriceProvider::Error>),

    #[error("No transactions generated")]
    NoTransactionsGenerated,

    #[error("No transactions approved")]
    NoTransactionsApproved,
}
