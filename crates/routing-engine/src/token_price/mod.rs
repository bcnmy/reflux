use std::error::Error;
use std::fmt::Debug;

use async_trait::async_trait;

pub use coingecko::CoingeckoClient;

mod coingecko;
pub mod utils;

#[async_trait]
pub trait TokenPriceProvider: Debug + Send + Sync {
    type Error: Error + Debug + Send + Sync;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error>;
}
