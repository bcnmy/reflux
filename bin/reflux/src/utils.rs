use std::sync::Arc;

use log::{debug, info};
use thiserror::Error;
use tokio::sync::Mutex;

use config::Config;
use routing_engine::{CostType, Route, token_price};
use routing_engine::source::RouteSource;
use routing_engine::token_price::TokenPriceProvider;
use routing_engine::token_price::utils::Errors;

const TOKEN_AMOUNT_USD_LOWER_BOUND: f64 = 0.0;
const TOKEN_AMOUNT_USD_UPPER_BOUND: f64 = 25.0;
const DIFFERENCE_THRESHOLD_USD: f64 = 0.5;

pub async fn find_lowest_transfer_amount_usd<
    Source: RouteSource,
    PriceProvider: TokenPriceProvider,
>(
    config: &Config,
    from_chain_id: u32,
    to_chain_id: u32,
    from_token_id: &String,
    to_token_id: &String,
    is_smart_contract_deposit: bool,
    source: &Source,
    token_price_provider: Arc<Mutex<PriceProvider>>,
) -> Result<f64, FindLowestTransferAmountErr<Source, PriceProvider>> {
    debug!(
        "Finding lowest transfer amount in USD for token {} on chain {} to token {} on chain {}",
        from_token_id, from_chain_id, to_token_id, to_chain_id
    );

    let mut low = TOKEN_AMOUNT_USD_LOWER_BOUND;
    let mut high = TOKEN_AMOUNT_USD_UPPER_BOUND;

    loop {
        let route = Route::build(
            config,
            &from_chain_id,
            &to_chain_id,
            from_token_id,
            to_token_id,
            is_smart_contract_deposit,
        )?;

        if high - low < DIFFERENCE_THRESHOLD_USD {
            info!("Found lowest transfer amount in USD: {} for route {}", high, route);
            break Ok(high);
        }

        debug!("Trying with low: {} and high: {} for route {}", low, high, route);

        let mid = (low + high) / 2.0;

        let from_token_amount_in_wei = token_price::utils::get_token_amount_from_value_in_usd(
            &config,
            &token_price_provider.lock().await,
            from_token_id,
            from_chain_id,
            &mid,
        )
        .await
        .map_err(FindLowestTransferAmountErr::GetTokenAmountFromValueInUsdErr)?;

        let result = source
            .fetch_least_cost_route_and_cost_in_usd(
                &route,
                &from_token_amount_in_wei,
                None,
                None,
                &CostType::Fee,
            )
            .await;

        if result.is_err() {
            low = mid;
        } else {
            high = mid;
        }
    }
}

#[derive(Error, Debug)]
pub enum FindLowestTransferAmountErr<T: RouteSource, U: TokenPriceProvider> {
    #[error("Failed to build route")]
    RouteBuildErr(#[from] routing_engine::RouteError),

    #[error("Failed to fetch route cost")]
    FetchRouteCostErr(T::FetchRouteCostError),

    #[error("Failed to get token amount from value in usd")]
    GetTokenAmountFromValueInUsdErr(Errors<U::Error>),
}
