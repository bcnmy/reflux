use derive_more::{Display, From};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct BungeeResponse<T> {
    pub success: bool,
    pub result: T,
}

// GET /quote
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteRequest {
    pub from_chain_id: u32,
    pub from_token_address: String,
    pub to_chain_id: u32,
    pub to_token_address: String,
    pub from_amount: String,
    pub user_address: String,
    pub recipient: String,
    pub unique_routes_per_bridge: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct BungeeTokenResponse {
    pub name: Option<String>,
    pub address: Option<String>,
    pub icon: Option<String>,
    pub decimals: Option<i8>,
    pub symbol: Option<String>,
    pub chain_id: Option<i32>,
    pub logo_URI: Option<String>,
    pub chain_agnostic_id: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponse {
    pub routes: Vec<GetQuoteResponseRoute>,
    pub from_chain_id: Option<u32>,
    pub from_asset: Option<BungeeTokenResponse>,
    pub to_chain_id: Option<u32>,
    pub to_asset: Option<BungeeTokenResponse>,
    pub refuel: Option<GetQuoteResponseRefuel>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponseRoute {
    pub route_id: String,
    pub is_only_swap_route: bool,
    pub from_amount: String,
    pub to_amount: String,
    #[serde(skip_serializing)]
    pub used_bridge_names: Vec<String>,
    pub total_user_tx: u32,
    pub total_gas_fees_in_usd: f64,
    pub recipient: String,
    pub sender: String,
    pub received_value_in_usd: Option<f64>,
    pub input_value_in_usd: Option<f64>,
    pub output_value_in_usd: Option<f64>,
    pub service_time: u32,
    pub max_service_time: u32,
    #[serde(skip_serializing)]
    pub integrator_fee: GetQuoteResponseRouteIntegratorFee,
    pub t2b_receiver_address: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponseRouteIntegratorFee {
    pub fee_taker_address: Option<String>,
    pub amount: Option<String>,
    pub asset: BungeeTokenResponse,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponseRefuel {
    pub from_amount: String,
    pub to_amount: String,
    pub gas_fee: GetQuoteResponseRefuelGasFee,
    pub recipient: String,
    pub service_time: u32,
    pub from_asset: BungeeTokenResponse,
    pub to_asset: BungeeTokenResponse,
    pub from_chain_id: u32,
    pub to_chain_id: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponseRefuelGasFee {
    pub gas_limit: u32,
    pub fee_in_usd: f64,
    pub asset: BungeeTokenResponse,
    pub gas_amount: String,
}

// Errors
#[derive(Debug, Display, From)]
pub enum BungeeClientError {
    #[display(
        fmt = "Error while deserializing response: Deserialization error: {}. Response: {}",
        _1,
        _0
    )]
    DeserializationError(String, serde_json::Error),

    #[display(fmt = "Error while making request: Request error: {}", _0)]
    RequestError(reqwest::Error),

    #[display(fmt = "No route returned by Bungee API")]
    NoRouteError,
}
