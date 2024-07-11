use std::collections::HashMap;
use std::sync::Arc;

use axum::{extract::Query, http::StatusCode, Json, response::IntoResponse, Router, routing::get};
use serde_json::json;

use account_aggregation::{service::AccountAggregationService, types};
use routing_engine::routing_engine::RoutingEngine;
use routing_engine::settlement_engine::SettlementEngine;
use routing_engine::source::RouteSource;
use routing_engine::token_price::TokenPriceProvider;

pub struct ServiceController<Source: RouteSource, PriceProvider: TokenPriceProvider> {
    account_service: Arc<AccountAggregationService>,
    routing_engine: Arc<RoutingEngine>,
    settlement_engine: Arc<SettlementEngine<Source, PriceProvider>>,
    token_chain_map: HashMap<String, HashMap<u32, bool>>,
    chain_supported: Vec<(u32, String)>,
    token_supported: Vec<String>,
}

impl<Source: RouteSource + 'static, PriceProvider: TokenPriceProvider + 'static>
    ServiceController<Source, PriceProvider>
{
    pub fn new(
        account_service: Arc<AccountAggregationService>,
        routing_engine: Arc<RoutingEngine>,
        settlement_engine: Arc<SettlementEngine<Source, PriceProvider>>,
        token_chain_map: HashMap<String, HashMap<u32, bool>>,
        chain_supported: Vec<(u32, String)>,
        token_supported: Vec<String>,
    ) -> Self {
        Self {
            account_service,
            routing_engine,
            settlement_engine,
            token_chain_map,
            chain_supported,
            token_supported,
        }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/", get(|| async { Self::status().await }))
            .route("/api/health", get(|| async { Self::status().await }))
            .route(
                "/api/account",
                get({
                    // todo: @ankurdubey521 should we use path instead of query here?
                    // move |Path(account): Path<String>| async move {
                    //     ServiceController::get_account(account_service, account).await
                    // }
                    let account_service = self.account_service.clone();
                    move |Query(query): Query<types::UserAccountMappingQuery>| async move {
                        Self::get_account(account_service, query).await
                    }
                }),
            )
            .route(
                "/api/account",
                axum::routing::post({
                    let account_service = self.account_service.clone();
                    move |Json(payload): Json<types::RegisterAccountPayload>| async move {
                        Self::register_user_account(account_service, payload).await
                    }
                }),
            )
            .route(
                "/api/account",
                axum::routing::patch({
                    let account_service = self.account_service.clone();
                    move |Json(payload): Json<types::AddAccountPayload>| async move {
                        Self::add_account(account_service, payload).await
                    }
                }),
            )
            .route(
                "/api/config",
                get({
                    let chain_supported = self.chain_supported.clone();
                    let token_supported = self.token_supported.clone();
                    move || async move { Self::get_config(chain_supported, token_supported) }
                }),
            )
            .route(
                "/api/balance",
                get({
                    let account_service = self.account_service.clone();
                    move |Query(query): Query<types::UserAccountMappingQuery>| async move {
                        Self::get_balance(account_service, query).await
                    }
                }),
            )
            .route(
                "/api/get_best_path",
                get({
                    let routing_engine = self.routing_engine.clone();
                    let settlement_engine = self.settlement_engine.clone();
                    let token_chain_map = self.token_chain_map.clone();

                    move |Query(query): Query<types::PathQuery>| async move {
                        Self::get_best_path(
                            routing_engine,
                            settlement_engine,
                            token_chain_map,
                            query,
                        )
                        .await
                    }
                }),
            )
    }

    /// Health check endpoint
    pub async fn status() -> impl IntoResponse {
        let response = json!({
            "message": "Service is running...",
            "status": "ok"
        });
        (StatusCode::OK, Json(response))
    }

    /// Get user accounts
    pub async fn get_account(
        account_service: Arc<AccountAggregationService>,
        query: types::UserAccountMappingQuery,
    ) -> impl IntoResponse {
        match account_service.get_user_id(&query.account).await {
            Ok(Some(user_id)) => match account_service.get_user_accounts(&user_id).await {
                Ok(Some(accounts)) => {
                    (StatusCode::OK, Json(json!({ "user_id": user_id, "accounts": accounts })))
                }
                Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Accounts not found" }))),
                Err(err) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err.to_string() })))
                }
            },
            Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "User not found" }))),
            Err(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err.to_string() })))
            }
        }
    }

    /// Register user account
    pub async fn register_user_account(
        account_service: Arc<AccountAggregationService>,
        payload: types::RegisterAccountPayload,
    ) -> impl IntoResponse {
        match account_service.register_user_account(payload).await {
            Ok(_) => {
                let response = json!({ "message": "User account registered successfully" });
                (StatusCode::OK, Json(response))
            }
            Err(err) => {
                let response = json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
            }
        }
    }

    /// Add account to user
    pub async fn add_account(
        account_service: Arc<AccountAggregationService>,
        payload: types::AddAccountPayload,
    ) -> impl IntoResponse {
        match account_service.add_account(payload).await {
            Ok(_) => {
                let response = json!({ "message": "Account added successfully" });
                (StatusCode::OK, Json(response))
            }
            Err(err) => {
                let response = json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
            }
        }
    }

    /// Get all supported chains and tokens
    pub fn get_config(
        chain_supported: Vec<(u32, String)>,
        token_supported: Vec<String>,
    ) -> impl IntoResponse {
        let response = json!({
            "chains": chain_supported,
            "tokens": token_supported
        });
        (StatusCode::OK, Json(response))
    }

    /// Get user account balance
    pub async fn get_balance(
        account_service: Arc<AccountAggregationService>,
        query: types::UserAccountMappingQuery,
    ) -> impl IntoResponse {
        match account_service.get_user_accounts_balance(&query.account).await {
            Ok(balances) => {
                // for loop to add the balance in USD
                let total_balance =
                    balances.iter().fold(0.0, |acc, balance| acc + balance.amount_in_usd);
                let response = json!({
                    "total_balance": total_balance,
                    "balances": balances
                });
                (StatusCode::OK, Json(response))
            }
            Err(err) => {
                let response = json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
            }
        }
    }

    /// Get best cost path for asset consolidation
    pub async fn get_best_path(
        routing_engine: Arc<RoutingEngine>,
        settlement_engine: Arc<SettlementEngine<Source, PriceProvider>>,
        token_chain_map: HashMap<String, HashMap<u32, bool>>,
        query: types::PathQuery,
    ) -> impl IntoResponse {
        // Check for the supported chain and token
        match token_chain_map.get(&query.to_token) {
            Some(chain_supported) => match chain_supported.get(&query.to_chain) {
                Some(supported) => {
                    if !supported {
                        let response = json!({ "error": "Token not supported on chain" });
                        return (StatusCode::BAD_REQUEST, Json(response));
                    }
                }
                None => {
                    let response = json!({ "error": "Chain not supported for token" });
                    return (StatusCode::BAD_REQUEST, Json(response));
                }
            },
            None => {
                let response = json!({ "error": "Token not supported" });
                return (StatusCode::BAD_REQUEST, Json(response));
            }
        }

        let routes_result = routing_engine
            .get_best_cost_paths(&query.account, query.to_chain, &query.to_token, query.to_value)
            .await;

        if let Err(err) = routes_result {
            let response = json!({ "error": err.to_string() });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(response));
        }

        let transactions_result =
            settlement_engine.generate_transactions(routes_result.unwrap()).await;

        if let Err(err) = transactions_result {
            let response = json!({ "error": err.to_string() });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(response));
        }

        let response = json!({ "routes": transactions_result.unwrap() });
        (StatusCode::OK, Json(response))
    }
}
