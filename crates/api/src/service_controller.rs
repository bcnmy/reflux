use account_aggregation::AccountAggregationService;
use axum::{extract::Query, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct AddressQuery {
    address: String,
}

#[derive(Deserialize)]
pub struct RegisterAccount {
    address: String,
    account_type: String,
    chain_id: String,
    is_enabled: bool,
}

#[derive(Deserialize)]
pub struct AddAccount {
    user_id: String,
    new_account: String,
    account_type: String,
    chain_id: String,
    is_enabled: bool,
}

pub struct ServiceController<T: storage::db_provider::DBProvider + Send + Sync + Clone + 'static> {
    account_service: Arc<AccountAggregationService<T>>,
}

impl<T: storage::db_provider::DBProvider + Send + Sync + Clone + 'static> ServiceController<T> {
    pub fn new(account_service: AccountAggregationService<T>) -> Self {
        Self { account_service: Arc::new(account_service) }
    }

    pub fn router(self) -> Router {
        let account_service = self.account_service.clone();

        Router::new()
            .route("/", get(ServiceController::<T>::status))
            .route("/api/health", get(ServiceController::<T>::status))
            .route(
                "/api/account",
                get({
                    let account_service = account_service.clone();
                    move |Query(query): Query<AddressQuery>| async move {
                        ServiceController::<T>::get_account(account_service.clone(), query).await
                    }
                }),
            )
            .route(
                "/api/register_account",
                axum::routing::post({
                    let account_service = account_service.clone();
                    move |Json(payload): Json<RegisterAccount>| async move {
                        ServiceController::<T>::register_user_account(
                            account_service.clone(),
                            payload,
                        )
                        .await
                    }
                }),
            )
            .route(
                "/api/add_account",
                axum::routing::post({
                    let account_service = account_service.clone();
                    move |Json(payload): Json<AddAccount>| async move {
                        ServiceController::<T>::add_account(account_service.clone(), payload).await
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
        account_service: Arc<AccountAggregationService<T>>,
        query: AddressQuery,
    ) -> impl IntoResponse {
        match account_service.get_user_accounts(&query.address).await {
            Ok(accounts) => {
                let response = json!({ "accounts": accounts });
                (StatusCode::OK, Json(response))
            }
            Err(err) => {
                let response = json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
            }
        }
    }

    /// Register user account
    pub async fn register_user_account(
        account_service: Arc<AccountAggregationService<T>>,
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
        account_service: Arc<AccountAggregationService<T>>,
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
}
