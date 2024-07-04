use std::str::FromStr;

use log::{error, info};
use reqwest;
use reqwest::header;
use ruint::aliases::U256;
use ruint::Uint;
use thiserror::Error;

use types::*;

use crate::{CostType, Route};
use crate::source::{EthereumTransaction, RequiredApprovalDetails, RouteSource};

mod types;

#[derive(Debug)]
pub struct BungeeClient {
    client: reqwest::Client,
    base_url: String,
}

impl BungeeClient {
    pub fn new<'config>(
        base_url: &'config String,
        api_key: &'config String,
    ) -> Result<Self, header::InvalidHeaderValue> {
        let mut headers = header::HeaderMap::new();
        headers.insert("API-KEY", header::HeaderValue::from_str(api_key)?);

        Ok(BungeeClient {
            client: reqwest::Client::builder().default_headers(headers).build().unwrap(),
            base_url: base_url.clone(),
        })
    }

    async fn get_quote(
        &self,
        params: GetQuoteRequest,
    ) -> Result<BungeeResponse<GetQuoteResponse>, BungeeClientError> {
        info!("Fetching quote from bungee for {:?}", params);

        let response =
            self.client.get(self.base_url.to_owned() + "/quote").query(&params).send().await?;
        let raw_text = response.text().await?;

        serde_json::from_str(&raw_text)
            .map_err(|err| BungeeClientError::DeserializationError(raw_text, err))
    }

    async fn build_tx(
        &self,
        params: BuildTxRequest,
    ) -> Result<BungeeResponse<BuildTxResponse>, BungeeClientError> {
        let response =
            self.client.post(self.base_url.to_owned() + "/build-tx").json(&params).send().await?;
        let raw_text = response.text().await?;

        serde_json::from_str(&raw_text)
            .map_err(|err| BungeeClientError::DeserializationError(raw_text, err))
    }
}

// Errors
#[derive(Debug, Error)]
pub enum BungeeClientError {
    #[error("Error while deserializing response: {0}")]
    DeserializationError(String, serde_json::Error),

    #[error("Error while Serializing Body: {0:?} with error {1:?}")]
    BodySerializationError(BuildTxRequest, serde_json::Error),

    #[error("Error while making request: Request error: {}", _0)]
    RequestError(#[from] reqwest::Error),

    #[error("No route returned by Bungee API")]
    NoRouteError,
}

const ADDRESS_ZERO: &'static str = "0x0000000000000000000000000000000000000000";

#[derive(Debug, Error)]
pub enum BungeeFetchRouteCostError {
    #[error("Configuration Missing for token {} on chain {}", _1, _0)]
    MissingChainForTokenInConfigError(u32, String),

    #[error("Error while making request: Request error: {}", _0)]
    BungeeClientError(#[from] BungeeClientError),

    #[error("Failure indicated in bungee response")]
    FailureIndicatedInResponseError(),

    #[error("No valid routes returned by Bungee API")]
    NoValidRouteError(),

    #[error("The estimation type {} is not implemented", _0)]
    EstimationTypeNotImplementedError(#[from] CostType),
}

#[derive(Error, Debug)]
pub enum GenerateRouteTransactionsError {
    #[error("Error while fetching least route and cost in USD: {0}")]
    FetchRouteCostError(#[from] BungeeFetchRouteCostError),

    #[error("Error while calling build transaction api: {0}")]
    BungeeClientError(#[from] BungeeClientError),

    #[error("Error while parsing U256: {0}")]
    InvalidU256Error(String),
}

impl RouteSource for BungeeClient {
    type FetchRouteCostError = BungeeFetchRouteCostError;

    type GenerateRouteTransactionsError = GenerateRouteTransactionsError;

    type BaseRouteType = serde_json::Value;

    async fn fetch_least_route_and_cost_in_usd(
        &self,
        route: &Route<'_>,
        from_token_amount: &U256,
        sender_address: Option<&String>,
        recipient_address: Option<&String>,
        estimation_type: &CostType,
    ) -> Result<(Self::BaseRouteType, f64), Self::FetchRouteCostError> {
        info!("Fetching least route cost in USD for route {:?} with token amount {} and estimation type {}", route, from_token_amount, estimation_type);

        // Build GetQuoteRequest
        let from_token = route.from_token.by_chain.get(&route.from_chain.id);
        if from_token.is_none() {
            error!("Missing chain for token {} in config", route.from_token.symbol);
            return Err(BungeeFetchRouteCostError::MissingChainForTokenInConfigError(
                route.from_chain.id,
                route.from_token.symbol.clone(),
            ));
        }

        let from_token = from_token.unwrap();

        let to_token = route.to_token.by_chain.get(&route.to_chain.id);
        if let None = to_token {
            error!("Missing chain for token {} in config", route.to_token.symbol);
            return Err(BungeeFetchRouteCostError::MissingChainForTokenInConfigError(
                route.to_chain.id,
                route.to_token.symbol.clone(),
            ));
        }
        let to_token = to_token.unwrap();

        let request = GetQuoteRequest {
            from_chain_id: route.from_chain.id,
            from_token_address: from_token.address.clone(),
            to_chain_id: route.to_chain.id,
            to_token_address: to_token.address.clone(),
            from_amount: from_token_amount.to_string(),
            user_address: sender_address.unwrap_or(&ADDRESS_ZERO.to_string()).clone(),
            recipient: recipient_address.unwrap_or(&ADDRESS_ZERO.to_string()).clone(),
            unique_routes_per_bridge: false,
            is_contract_call: route.is_smart_contract_deposit,
        };

        // Get quote
        let response = self.get_quote(request).await?;
        if !response.success {
            return Err(BungeeFetchRouteCostError::FailureIndicatedInResponseError());
        }

        let get_route_fee = |route: &serde_json::Value| {
            let total_gas_fees_in_usd = route.get("totalGasFeesInUsd")?.as_f64()?;
            let input_value_in_usd = route.get("inputValueInUsd")?.as_f64()?;
            let output_value_in_usd = route.get("outputValueInUsd")?.as_f64()?;

            match estimation_type {
                CostType::Fee => {
                    Some(total_gas_fees_in_usd + input_value_in_usd - output_value_in_usd)
                }
            }
        };

        // Find the minimum cost across all routes
        let (route_costs_in_usd, failed): (Vec<(serde_json::Value, Option<f64>)>, _) = response
            .result
            .routes
            .into_iter()
            .map(|route| {
                let fee = get_route_fee(&route);
                (route, fee)
            })
            .partition(|(_, r)| r.is_some());

        let route_costs_in_usd: Vec<(serde_json::Value, f64)> =
            route_costs_in_usd.into_iter().map(|(route, r)| (route, r.unwrap())).collect();

        if failed.len() > 0 {
            let failed: Vec<serde_json::Value> =
                failed.into_iter().map(|(route, r)| route).collect();
            error!("Error while calculating route costs in usd: {:?}", failed);
        }

        if route_costs_in_usd.len() == 0 {
            error!("No valid routes returned by Bungee API for route {:?}", route);
            return Err(BungeeFetchRouteCostError::NoValidRouteError());
        }

        info!("Route costs in USD: {:?}", route_costs_in_usd.len());

        let min_cost_route_and_fee =
            route_costs_in_usd.into_iter().min_by(|a, b| a.1.total_cmp(&b.1));

        return if let Some(min_cost_route_and_fee) = min_cost_route_and_fee {
            Ok(min_cost_route_and_fee)
        } else {
            error!("No valid routes returned after applying min function");
            Err(BungeeFetchRouteCostError::NoValidRouteError())
        };
    }

    async fn generate_route_transactions(
        &self,
        route: &Route<'_>,
        amount: &U256,
        sender_address: &String,
        recipient_address: &String,
    ) -> Result<
        (Vec<EthereumTransaction>, Vec<RequiredApprovalDetails>),
        Self::GenerateRouteTransactionsError,
    > {
        info!(
            "Generating cheapest route transactions for route {:?} with amount {}",
            route, amount
        );

        let (bungee_route, _) = self
            .fetch_least_route_and_cost_in_usd(
                route,
                amount,
                Some(sender_address),
                Some(recipient_address),
                &CostType::Fee,
            )
            .await?;

        info!("Retrieved bungee route {:?} for route {:?}", bungee_route, route);

        let tx = self.build_tx(BuildTxRequest { route: bungee_route }).await?;
        if !tx.success {
            error!("Failure indicated in bungee response");

            return Err(GenerateRouteTransactionsError::FetchRouteCostError(
                BungeeFetchRouteCostError::FailureIndicatedInResponseError(),
            ));
        }

        let tx = tx.result;
        info!("Returned transaction from bungee {:?}", tx);

        let transactions = vec![EthereumTransaction {
            to: tx.tx_target,
            value: Uint::from_str(&tx.value).map_err(|err| {
                error!("Error while parsing tx data: {}", err);
                GenerateRouteTransactionsError::InvalidU256Error(tx.value)
            })?,
            calldata: tx.tx_data,
        }];

        info!("Generated transactions {:?}", transactions);

        let approvals = vec![RequiredApprovalDetails {
            chain_id: tx.chain_id,
            token_address: tx.approval_data.approval_token_address,
            spender: tx.approval_data.owner,
            amount: Uint::from_str(&tx.approval_data.minimum_approval_amount).map_err(|err| {
                error!("Error while parsing approval data: {}", err);
                GenerateRouteTransactionsError::InvalidU256Error(
                    tx.approval_data.minimum_approval_amount,
                )
            })?,
        }];

        info!("Generated approvals {:?}", approvals);

        Ok((transactions, approvals))
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use ruint::Uint;

    use config::Config;
    use config::get_sample_config;

    use crate::{BungeeClient, CostType, Route};
    use crate::source::bungee::types::GetQuoteRequest;
    use crate::source::RouteSource;

    fn setup() -> (Config, BungeeClient) {
        let config = get_sample_config();

        let bungee_client = BungeeClient::new(
            &"https://api.socket.tech/v2".to_string(),
            &env::var("BUNGEE_API_KEY").unwrap().to_string(),
        )
        .unwrap();

        return (config, bungee_client);
    }

    #[tokio::test]
    async fn test_fetch_quote() {
        let (_, client) = setup();

        let response = client
            .get_quote(GetQuoteRequest {
                from_chain_id: 1,
                from_token_address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string(),
                to_chain_id: 42161,
                to_token_address: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_string(),
                from_amount: "100000000".to_string(),
                user_address: "0x0000000000000000000000000000000000000000".to_string(),
                recipient: "0x0000000000000000000000000000000000000000".to_string(),
                is_contract_call: false,
                unique_routes_per_bridge: false,
            })
            .await
            .unwrap();

        assert_eq!(response.success, true);
        assert_eq!(response.result.routes.len() > 0, true);
    }

    #[tokio::test]
    async fn test_fetch_least_cost_route() {
        let (config, client) = setup();

        let route = Route {
            from_chain: &config.chains.get(&1).unwrap(),
            to_chain: &config.chains.get(&42161).unwrap(),
            from_token: &config.tokens.get(&"USDC".to_string()).unwrap(),
            to_token: &config.tokens.get(&"USDC".to_string()).unwrap(),
            is_smart_contract_deposit: false,
        };
        let (_, least_route_cost) = client
            .fetch_least_route_and_cost_in_usd(
                &route,
                &Uint::from(100000000),
                None,
                None,
                &CostType::Fee,
            )
            .await
            .unwrap();

        assert_eq!(least_route_cost > 0.0, true);
    }

    #[tokio::test]
    async fn test_generate_route_transactions() {
        let (config, client) = setup();

        let route = Route {
            from_chain: &config.chains.get(&1).unwrap(),
            to_chain: &config.chains.get(&42161).unwrap(),
            from_token: &config.tokens.get(&"USDC".to_string()).unwrap(),
            to_token: &config.tokens.get(&"USDC".to_string()).unwrap(),
            is_smart_contract_deposit: false,
        };

        let address = "0x90f05C1E52FAfB4577A4f5F869b804318d56A1ee".to_string();

        let token_amount = Uint::from(100000000);
        let result =
            client.generate_route_transactions(&route, &token_amount, &address, &address).await;

        assert!(result.is_ok());

        let (transactions, approvals) = result.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(approvals.len(), 1);
    }
}
