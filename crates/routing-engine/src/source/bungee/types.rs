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
    pub is_contract_call: bool,
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
    pub routes: Vec<serde_json::Value>,
    pub from_chain_id: Option<u32>,
    pub from_asset: Option<BungeeTokenResponse>,
    pub to_chain_id: Option<u32>,
    pub to_asset: Option<BungeeTokenResponse>,
    pub refuel: Option<GetQuoteResponseRefuel>,
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

// POST /build-tx
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildTxRequest {
    pub(crate) route: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildTxResponse {
    pub user_tx_type: String,
    pub tx_target: String,
    pub chain_id: u32,
    pub tx_data: String,
    pub tx_type: String,
    pub value: String,
    pub total_user_tx: Option<u32>,
    pub approval_data: BuildTxResponseApprovalData,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildTxResponseApprovalData {
    pub minimum_approval_amount: String,
    pub approval_token_address: String,
    pub allowance_target: String,
    pub owner: String,
}
