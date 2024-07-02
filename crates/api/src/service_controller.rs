use std::collections::HashMap;
use std::sync::Arc;

use axum::{extract::Query, http::StatusCode, Json, response::IntoResponse, Router, routing::get};
use serde_json::json;

use account_aggregation::{service::AccountAggregationService, types};
use routing_engine::routing_engine::RoutingEngine;

pub struct ServiceController {
    account_service: Arc<AccountAggregationService>,
    routing_engine: Arc<RoutingEngine>,
    token_supported: HashMap<String, HashMap<u32, bool>>,
}

impl ServiceController {
    pub fn new(
        account_service: AccountAggregationService,
        routing_engine: Arc<RoutingEngine>,
        token_supported: HashMap<String, HashMap<u32, bool>>,
    ) -> Self {
        Self { account_service: Arc::new(account_service), routing_engine, token_supported }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/", get(ServiceController::status))
            .route("/api/health", get(ServiceController::status))
            .route(
                "/api/account",
                get({
                    let account_service = self.account_service.clone();
                    move |Query(query): Query<types::UserAccountMappingQuery>| async move {
                        ServiceController::get_account(account_service, query).await
                    }
                }),
            )
            .route(
                "/api/register_account",
                axum::routing::post({
                    let account_service = self.account_service.clone();
                    move |Json(payload): Json<types::RegisterAccountPayload>| async move {
                        ServiceController::register_user_account(account_service, payload).await
                    }
                }),
            )
            .route(
                "/api/add_account",
                axum::routing::post({
                    let account_service = self.account_service.clone();
                    move |Json(payload): Json<types::AddAccountPayload>| async move {
                        ServiceController::add_account(account_service, payload).await
                    }
                }),
            )
            .route(
                "/api/get_best_path",
                get({
                    let routing_engine = self.routing_engine.clone();
                    let token_supported = self.token_supported.clone();
                    move |Query(query): Query<types::PathQuery>| async move {
                        ServiceController::get_best_path(routing_engine, token_supported, query)
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
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "User not found", "accounts": [query.account] })),
            ),
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

    /// Get best cost path for asset consolidation
    pub async fn get_best_path(
        routing_engine: Arc<RoutingEngine>,
        token_supported: HashMap<String, HashMap<u32, bool>>,
        query: types::PathQuery,
    ) -> impl IntoResponse {
        // Check for the supported chain and token
        match token_supported.get(&query.to_token) {
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

        match routing_engine
            .get_best_cost_paths(&query.account, query.to_chain, &query.to_token, query.to_value)
            .await
        {
            Ok(routes) => {
                let response = json!({ "routes": "routes" });
                (StatusCode::OK, Json(response))
            }
            Err(err) => {
                let response = json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
            }
        }
    }
}
