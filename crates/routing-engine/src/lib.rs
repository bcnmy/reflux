use std::sync::Arc;

pub use alloy::providers::Provider;
pub use alloy::transports::Transport;
use derive_more::Display;
use thiserror::Error;

use config::{ChainConfig, TokenConfig};
use config::config::{BucketConfig, Config};
pub use indexer::Indexer;
pub use source::bungee::BungeeClient;
pub use token_price::CoingeckoClient;

pub mod routing_engine;
pub mod token_price;

pub mod blockchain;
pub mod estimator;
pub mod indexer;
pub mod settlement_engine;
pub mod source;

#[derive(Debug, Error, Display)]
pub enum CostType {
    Fee,
    // BridgingTime,
}

#[derive(Debug, Clone)]
pub struct Route {
    from_chain: Arc<ChainConfig>,
    to_chain: Arc<ChainConfig>,
    from_token: Arc<TokenConfig>,
    to_token: Arc<TokenConfig>,
    is_smart_contract_deposit: bool,
}

impl Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Route: chain: {} -> {}, token: {} -> {}, is_smart_contract_deposit: {}",
            self.from_chain.id,
            self.to_chain.id,
            self.from_token.symbol,
            self.to_token.symbol,
            self.is_smart_contract_deposit
        )
    }
}

impl Route {
    pub fn build(
        config: &Config,
        from_chain_id: &u32,
        to_chain_id: &u32,
        from_token_id: &String,
        to_token_id: &String,
        is_smart_contract_deposit: bool,
    ) -> Result<Route, RouteError> {
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
            from_chain: Arc::clone(from_chain.unwrap()),
            to_chain: Arc::clone(to_chain.unwrap()),
            from_token: Arc::clone(from_token.unwrap()),
            to_token: Arc::clone(to_token.unwrap()),
            is_smart_contract_deposit,
        })
    }

    pub fn build_from_bucket(bucket: &BucketConfig, config: &Config) -> Result<Route, RouteError> {
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
pub struct BridgeResult {
    route: Route,
    source_amount_in_usd: f64,
    from_address: String,
    to_address: String,
}

impl Display for BridgeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BridgeResult: route: {}, source_amount_in_usd: {}, from_address: {}, to_address: {}",
            self.route, self.source_amount_in_usd, self.from_address, self.to_address
        )
    }
}

pub struct BridgeResultVecWrapper<'a>(&'a Vec<BridgeResult>);

impl Display for BridgeResultVecWrapper<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for bridge_result in self.0 {
            write!(f, "{}, ", bridge_result)?;
        }
        write!(f, "]")?;
        Ok(())
    }
}

impl BridgeResult {
    pub fn build(
        config: &Config,
        from_chain_id: &u32,
        to_chain_id: &u32,
        from_token_id: &String,
        to_token_id: &String,
        is_smart_contract_deposit: bool,
        source_amount_in_usd: f64,
        from_address: String,
        to_address: String,
    ) -> Result<BridgeResult, RouteError> {
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

#[cfg(test)]
mod test {
    use config::get_sample_config;

    fn assert_is_send(_: impl Send) {}

    #[test]
    fn test_route_must_be_send() {
        let config = get_sample_config();
        let route = super::Route::build(
            &config,
            &1,
            &42161,
            &"USDC".to_string(),
            &"USDT".to_string(),
            false,
        )
        .unwrap();

        assert_is_send(route);
    }

    #[test]
    fn test_bridge_result_must_be_send() {
        let config = get_sample_config();
        let bridge_result = super::BridgeResult::build(
            &config,
            &1,
            &42161,
            &"USDC".to_string(),
            &"USDT".to_string(),
            false,
            100.0,
            "0x123".to_string(),
            "0x456".to_string(),
        )
        .unwrap();

        assert_is_send(bridge_result);
    }
}
