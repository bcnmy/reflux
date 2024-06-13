use std::cmp::PartialEq;
use std::fmt::{Debug, Display};

use derive_more::{Display, From};
use num_bigint::BigUint;
use reqwest;
use reqwest::header;
use serde::de::StdError;
use serde::{Deserialize, Serialize};

use types::*;

use crate::source::{Calldata, RouteSource};
use crate::{EstimationType, Route};

mod types;

pub struct BungeeClient {
    client: reqwest::Client,
    base_url: String,
}

impl BungeeClient {
    fn new(
        config::BungeeConfig { base_url, api_key }: &config::BungeeConfig,
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

#[derive(Debug, Display, From)]
pub enum BungeeFetchRouteCostError {
    #[display(fmt = "Configuration Missing for token {} on chain {}", _1, _0)]
    MissingChainForTokenInConfigError(u32, String),

    #[display(fmt = "Error while making request: Request error: {}", _0)]
    BungeeClientError(BungeeClientError),

    #[display(fmt = "Failure indicated in bungee response")]
    FailureIndicatedInResponseError(),

    #[display(fmt = "No valid routes returned by Bungee API")]
    NoValidRouteError(),

    #[display(fmt = "The estimation type {} is not implemented", _0)]
    EstimationTypeNotImplementedError(EstimationType),
}

impl RouteSource for BungeeClient {
    type FetchRouteCostError = BungeeFetchRouteCostError;
    type GenerateRouteCalldataError = ();

    async fn fetch_route_cost_in_usd(
        &self,
        route: &Route<'_>,
        from_token_amount: BigUint,
        estimation_type: EstimationType,
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
                EstimationType::Cheapest => Some(
                    route.total_gas_fees_in_usd + route.output_value_in_usd?
                        - route.input_value_in_usd?,
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
        route: &Route<'_>,
    ) -> Result<Calldata, Self::GenerateRouteCalldataError> {
        unimplemented!()
    }
}
