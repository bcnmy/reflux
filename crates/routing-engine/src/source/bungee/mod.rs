use reqwest;
use reqwest::header;
use ruint::aliases::U256;
use thiserror::Error;

use config::config::BungeeConfig;
use types::*;

use crate::{CostType, Route};
use crate::source::{Calldata, RouteSource};

mod types;

#[derive(Debug)]
pub struct BungeeClient {
    client: reqwest::Client,
    base_url: String,
}

impl BungeeClient {
    pub(crate) fn new(
        BungeeConfig { base_url, api_key }: &BungeeConfig,
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
        let response =
            self.client.get(self.base_url.to_owned() + "/quote").query(&params).send().await?;
        let raw_text = response.text().await?;

        serde_json::from_str(&raw_text)
            .map_err(|err| BungeeClientError::DeserializationError(raw_text, err))
    }
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

impl RouteSource for BungeeClient {
    type FetchRouteCostError = BungeeFetchRouteCostError;
    type GenerateRouteCalldataError = ();

    async fn fetch_least_route_cost_in_usd(
        &self,
        route: &Route<'_>,
        from_token_amount: U256,
        estimation_type: &CostType,
    ) -> Result<f64, Self::FetchRouteCostError> {
        // Build GetQuoteRequest
        let from_token = route.from_token.by_chain.get(&route.from_chain.id);
        if from_token.is_none() {
            return Err(BungeeFetchRouteCostError::MissingChainForTokenInConfigError(
                route.from_chain.id,
                route.from_token.symbol.clone(),
            ));
        }

        let from_token = from_token.unwrap();

        let to_token = route.to_token.by_chain.get(&route.to_chain.id);
        if let None = to_token {
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
            user_address: ADDRESS_ZERO.to_string(),
            recipient: ADDRESS_ZERO.to_string(),
            unique_routes_per_bridge: false,
            is_contract_call: route.is_smart_contract_deposit,
        };

        // Get quote
        let response = self.get_quote(request).await?;
        if !response.success {
            return Err(BungeeFetchRouteCostError::FailureIndicatedInResponseError());
        }

        // Find the minimum cost across all routes
        let route_costs_in_usd: Vec<f64> = response
            .result
            .routes
            .iter()
            .map(|route| match estimation_type {
                CostType::Fee => Some(
                    route.total_gas_fees_in_usd + route.input_value_in_usd?
                        - route.output_value_in_usd?,
                ),
                _ => None,
            })
            .filter(|cost| cost.is_some())
            .map(|cost| cost.unwrap())
            .collect();

        if route_costs_in_usd.len() == 0 {
            return Err(BungeeFetchRouteCostError::NoValidRouteError());
        }

        Ok(route_costs_in_usd.into_iter().min_by(|a, b| a.total_cmp(b)).unwrap())
    }

    async fn generate_route_calldata(
        &self,
        _route: &Route<'_>,
    ) -> Result<Calldata, Self::GenerateRouteCalldataError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use ruint::Uint;

    use config::Config;

    use crate::source::RouteSource;
    use crate::{CostType, Route};
    use crate::source::bungee::BungeeClient;
    use crate::source::bungee::types::GetQuoteRequest;

    fn setup() -> (Config, BungeeClient) {
        // let config = config::Config::from_file("../../config.yaml").unwrap();
        let mut config = config::Config::from_yaml_str(
            r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
  - id: 42161
    name: Arbitrum
    is_enabled: true
tokens:
  - symbol: USDC
    is_enabled: true
    by_chain:
      1:
        is_enabled: true
        decimals: 6
        address: '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'
      42161:
        is_enabled: true
        decimals: 6
        address: '0xaf88d065e77c8cC2239327C5EDb3A432268e5831'
buckets:
  - from_chain_id: 1
    to_chain_id: 42161
    from_token: USDC
    to_token: USDC
    is_smart_contract_deposit_supported: false
    token_amount_from_usd: 1
    token_amount_to_usd: 10
bungee:
  base_url: https://api.socket.tech/v2
  api_key: <REDACTED>
covalent:
  base_url: 'https://api.bungee.exchange'
  api_key: 'my-api'
coingecko:
  base_url: 'https://api.coingecko.com'
  api_key: 'my-api'
infra:
  redis_url: 'redis://localhost:6379'
  rabbitmq_url: 'amqp://localhost:5672'
  mongo_url: 'mongodb://localhost:27017'
server:
  port: 8080
  host: 'localhost'
indexer_config:
    is_indexer: true
    indexer_update_topic: indexer_update
    indexer_update_message: message
        "#,
        )
        .unwrap();

        config.bungee.api_key = env::var("BUNGEE_API_KEY").unwrap();

        let bungee_client = BungeeClient::new(&config.bungee).unwrap();
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
        let least_route_cost = client
            .fetch_least_route_cost_in_usd(&route, Uint::from(100000000), &CostType::Fee)
            .await
            .unwrap();

        assert_eq!(least_route_cost > 0.0, true);
    }
}
