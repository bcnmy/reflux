use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct ApiResponse {
    pub data: ApiData,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ApiData {
    pub items: Vec<TokenData>,
    pub chain_id: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct TokenData {
    pub contract_ticker_symbol: Option<String>,
    pub balance: Option<String>,
    pub quote: Option<f64>,
    pub contract_decimals: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Balance {
    pub token: String,
    pub token_address: String,
    pub chain_id: u32,
    pub amount: f64,
    pub amount_in_usd: f64,
}
