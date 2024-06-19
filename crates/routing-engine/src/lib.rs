// use route_fee_bucket::RouteFeeBucket;
pub mod engine;
pub mod route_fee_bucket;
mod traits;

#[cfg(test)]
mod tests;
pub mod token_price;

use derive_more::{Display, From};

use config::config::{BucketConfig, ChainConfig, Config, TokenConfig};
pub use indexer::Indexer;

pub mod estimator;
pub mod indexer;
mod source;

#[derive(Debug, Display)]
enum CostType {
    Fee,
    BridgingTime,
}

pub struct Route<'a> {
    from_chain: &'a ChainConfig,
    to_chain: &'a ChainConfig,
    from_token: &'a TokenConfig,
    to_token: &'a TokenConfig,
    is_smart_contract_deposit: bool,
}

impl<'a> Route<'a> {
    pub fn build(bucket: &'a BucketConfig, config: &'a Config) -> Result<Route<'a>, RouteError> {
        let from_chain = config.chains.get(&bucket.from_chain_id);
        if from_chain.is_none() {
            return Err(RouteError::ChainNotFoundError(bucket.from_chain_id));
        }

        let to_chain = config.chains.get(&bucket.to_chain_id);
        if to_chain.is_none() {
            return Err(RouteError::ChainNotFoundError(bucket.to_chain_id));
        }

        let from_token = config.tokens.get(&bucket.from_token);
        if from_token.is_none() {
            return Err(RouteError::TokenNotFoundError(bucket.from_token.clone()));
        }

        let to_token = config.tokens.get(&bucket.to_token);
        if to_token.is_none() {
            return Err(RouteError::TokenNotFoundError(bucket.to_token.clone()));
        }

        Ok(Route {
            from_chain: from_chain.unwrap(),
            to_chain: to_chain.unwrap(),
            from_token: from_token.unwrap(),
            to_token: to_token.unwrap(),
            is_smart_contract_deposit: bucket.is_smart_contract_deposit_supported,
        })
    }
}

#[derive(Debug, Display, From)]
enum RouteError {
    #[display("Chain not found while building route: {}", _0)]
    ChainNotFoundError(u32),

    #[display("Token not found while building route: {}", _0)]
    TokenNotFoundError(String),
}
