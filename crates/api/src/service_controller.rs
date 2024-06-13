use account_aggregation::AccountAggregationService;
use axum::{extract::Query, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use routing_engine::engine::RoutingEngine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct AddressQuery {
    address: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RegisterAccount {
    address: String,
    account_type: String,
    chain_id: String,
    is_enabled: bool,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct AddAccount {
    user_id: String,
    new_account: String,
    account_type: String,
    chain_id: String,
    is_enabled: bool,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct PathQuery {
    user_id: String,
    to_chain: u32,
    to_token: String,
    to_value: f64,
}

pub struct ServiceController {
    account_service: Arc<AccountAggregationService>,
    routing_engine: Arc<RoutingEngine>,
}

impl ServiceController {
    pub fn new(account_service: AccountAggregationService, routing_engine: RoutingEngine) -> Self {
        Self {
            account_service: Arc::new(account_service),
            routing_engine: Arc::new(routing_engine),
        }
    }

    pub fn router(self) -> Router {
        let account_service = self.account_service.clone();
        let routing_engine = self.routing_engine.clone();

        Router::new()
            .route("/", get(ServiceController::status))
            .route("/api/health", get(ServiceController::status))
            .route(
                "/api/account",
                get({
                    let account_service = account_service.clone();
                    move |Query(query): Query<AddressQuery>| async move {
                        ServiceController::get_account(account_service.clone(), query).await
                    }
                }),
            )
            .route(
                "/api/register_account",
                axum::routing::post({
                    let account_service = account_service.clone();
                    move |Json(payload): Json<RegisterAccount>| async move {
                        ServiceController::register_user_account(account_service.clone(), payload)
                            .await
                    }
                }),
            )
            .route(
                "/api/add_account",
                axum::routing::post({
                    let account_service = account_service.clone();
                    move |Json(payload): Json<AddAccount>| async move {
                        ServiceController::add_account(account_service.clone(), payload).await
                    }
                }),
            )
            .route(
                "/api/get_best_path",
                get({
                    move |Query(query): Query<PathQuery>| {
                        let future = async move {
                            ServiceController::get_best_path(routing_engine.clone(), query).await
                        };
                        future
                    }
                }),
            )
    }

    /// Health check endpoint
    pub async fn status() -> impl IntoResponse {
        println!("Service is running...");
        let response = json!({
            "message": "Service is running...",
            "status": "ok"
        });
        (StatusCode::OK, Json(response))
    }

    /// Get user accounts
    pub async fn get_account(
        account_service: Arc<AccountAggregationService>,
        query: AddressQuery,
    ) -> impl IntoResponse {
        let user_id = account_service.get_user_id(&query.address).await;

        let response = match user_id {
            Some(user_id) => {
                let accounts = account_service.get_user_accounts(&user_id).await;
                json!({ "user_id": user_id, "accounts": accounts })
            }
            None => json!({ "error": "User not found" }),
        };

        (StatusCode::OK, Json(response))
    }

    /// Register user account
    pub async fn register_user_account(
        account_service: Arc<AccountAggregationService>,
        payload: RegisterAccount,
    ) -> impl IntoResponse {
        match account_service
            .register_user_account(
                payload.address,
                payload.account_type,
                payload.chain_id,
                payload.is_enabled,
            )
            .await
        {
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
        payload: AddAccount,
    ) -> impl IntoResponse {
        match account_service
            .add_account(
                payload.user_id,
                payload.new_account,
                payload.account_type,
                payload.chain_id,
                payload.is_enabled,
            )
            .await
        {
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
        query: PathQuery,
    ) -> impl IntoResponse {
        let routes = routing_engine
            .get_best_cost_path(
                &query.user_id,
                query.to_chain,
                &query.to_token,
                query.to_value,
            )
            .await;
        let response = json!({ "routes": routes });

        (StatusCode::OK, Json(response))
    }
}
