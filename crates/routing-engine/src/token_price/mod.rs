use std::error::Error;
use std::fmt::Debug;

pub use coingecko::CoingeckoClient;

mod coingecko;
pub mod utils;

pub trait TokenPriceProvider: Debug + Send + Sync {
    type Error: Error + Debug + Send + Sync;

    fn get_token_price(
        &self,
        token_symbol: &String,
    ) -> impl futures::Future<Output = Result<f64, Self::Error>>;
}
