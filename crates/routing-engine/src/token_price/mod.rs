use std::error::Error;
use std::fmt::Debug;

pub use coingecko::CoingeckoClient;

mod coingecko;
pub mod utils;

pub trait TokenPriceProvider: Debug {
    type Error: Error + Debug;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error>;
}

