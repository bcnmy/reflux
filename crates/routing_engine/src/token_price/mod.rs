use std::fmt::Display;

mod utils;

pub trait TokenPriceProvider {
    type Error: Display;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error>;
}
