use std::fmt::{Debug, Display};

pub mod utils;

pub trait TokenPriceProvider: Debug {
    type Error: Display + Debug;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error>;
}
