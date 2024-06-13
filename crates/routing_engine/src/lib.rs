use derive_more::Display;
use ruint;

use config;

pub mod estimator;
pub mod indexer;
mod source;
mod token_price;

#[derive(Debug, Display)]
enum CostType {
    Fee,
    BridgingTime,
}

pub struct Route<'a> {
    from_chain: &'a config::ChainConfig,
    to_chain: &'a config::ChainConfig,
    from_token: &'a config::TokenConfig,
    to_token: &'a config::TokenConfig,
}