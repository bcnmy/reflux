use derive_more::Display;
use thiserror::Error;

use config::config::{BucketConfig, ChainConfig, Config, TokenConfig};
pub use indexer::Indexer;
pub use source::bungee::BungeeClient;
pub use token_price::CoingeckoClient;

pub mod routing_engine;
pub mod token_price;

mod contracts;
pub mod estimator;
pub mod indexer;
mod settlement_engine;
mod source;

#[derive(Debug, Error, Display)]
pub enum CostType {
    Fee,
    // BridgingTime,
}

#[derive(Debug)]
pub struct Route<'config> {
    from_chain: &'config ChainConfig,
    to_chain: &'config ChainConfig,
    from_token: &'config TokenConfig,
    to_token: &'config TokenConfig,
    is_smart_contract_deposit: bool,
}

impl<'a> Route<'a> {
    pub fn build(
        config: &'a Config,
        from_chain_id: &u32,
        to_chain_id: &u32,
        from_token_id: &String,
        to_token_id: &String,
        is_smart_contract_deposit: bool,
    ) -> Result<Route<'a>, RouteError> {
        let from_chain = config.chains.get(from_chain_id);
        if from_chain.is_none() {
            return Err(RouteError::ChainNotFoundError(*from_chain_id));
        }

        let to_chain = config.chains.get(to_chain_id);
        if to_chain.is_none() {
            return Err(RouteError::ChainNotFoundError(*to_chain_id));
        }

        let from_token = config.tokens.get(from_token_id);
        if from_token.is_none() {
            return Err(RouteError::TokenNotFoundError(from_token_id.clone()));
        }

        let to_token = config.tokens.get(to_token_id);
        if to_token.is_none() {
            return Err(RouteError::TokenNotFoundError(to_token_id.clone()));
        }

        Ok(Route {
            from_chain: from_chain.unwrap(),
            to_chain: to_chain.unwrap(),
            from_token: from_token.unwrap(),
            to_token: to_token.unwrap(),
            is_smart_contract_deposit,
        })
    }

    pub fn build_from_bucket(
        bucket: &'a BucketConfig,
        config: &'a Config,
    ) -> Result<Route<'a>, RouteError> {
        Self::build(
            config,
            &bucket.from_chain_id,
            &bucket.to_chain_id,
            &bucket.from_token,
            &bucket.to_token,
            bucket.is_smart_contract_deposit_supported,
        )
    }
}

#[derive(Debug, Error)]
pub enum RouteError {
    #[error("Chain not found while building route: {}", _0)]
    ChainNotFoundError(u32),

    #[error("Token not found while building route: {}", _0)]
    TokenNotFoundError(String),
}

#[derive(Debug)]
pub struct BridgeResult<'config> {
    route: Route<'config>,
    source_amount_in_usd: f64,
    from_address: String,
    to_address: String,
}

impl BridgeResult<'_> {
    pub fn build<'config>(
        config: &'config Config,
        from_chain_id: &u32,
        to_chain_id: &u32,
        from_token_id: &String,
        to_token_id: &String,
        is_smart_contract_deposit: bool,
        source_amount_in_usd: f64,
        from_address: String,
        to_address: String,
    ) -> Result<BridgeResult<'config>, RouteError> {
        Ok(BridgeResult {
            route: Route::build(
                config,
                from_chain_id,
                to_chain_id,
                from_token_id,
                to_token_id,
                is_smart_contract_deposit,
            )?,
            source_amount_in_usd,
            from_address,
            to_address,
        })
    }
}
