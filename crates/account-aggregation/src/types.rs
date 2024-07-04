use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct CovalentApiResponse {
    pub data: CovalentApiData,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct CovalentApiData {
    pub items: Vec<CovalentTokenData>,
    pub chain_id: u32,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct CovalentTokenData {
    pub contract_ticker_symbol: Option<String>,
    pub balance: Option<String>,
    pub quote: Option<f64>,
    pub contract_decimals: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct TokenWithBalance {
    pub token: String,
    pub token_address: String,
    pub chain_id: u32,
    pub amount: f64,
    pub amount_in_usd: f64,
}

// User DB Model
#[derive(Deserialize, Serialize, Debug)]
pub struct User {
    pub user_id: String,
    pub accounts: Vec<Account>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Account {
    pub address: String,
    pub account_type: String,
    pub is_enabled: bool,
    pub tags: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct UserQuery {
    pub user_id: String,
}

// Account User Mapping DB
#[derive(Deserialize, Serialize, Debug)]
pub struct UserAccountMapping {
    pub user_id: String,
    pub account: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct UserAccountMappingQuery {
    pub account: String,
}

// Register Account Payload (same as Account)
#[derive(Deserialize, Serialize, Debug)]
pub struct RegisterAccountPayload {
    pub accounts: Vec<Account>,
}
// Add Account Payload (need to add user_id)
#[derive(Deserialize, Serialize, Debug)]
pub struct AddAccountPayload {
    pub user_id: Option<String>,
    pub account: Vec<Account>
}

// Path Query Model
#[derive(Deserialize, Serialize, Debug)]
pub struct PathQuery {
    pub account: String,
    pub to_chain: u32,
    pub to_token: String,
    pub to_value: f64,
}
