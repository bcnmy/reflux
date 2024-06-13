use account_aggregation::AccountAggregationService;
use axum::{extract::Query, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use storage::db_provider::DBProvider;

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

pub fn router<T: DBProvider + Send + Sync + Clone + 'static>(db_provider: T) -> Router {
    let db_provider = Arc::new(db_provider);
    Router::new()
        .route("/", get(status))
        .route("/api/health", get(status))
        .route(
            "/api/account",
            get({
                let db_provider = db_provider.clone();
                move |Query(query): Query<AddressQuery>| async move {
                    get_account(db_provider.clone(), query).await
                }
            }),
        )
        .route(
            "/api/register_account",
            axum::routing::post({
                let db_provider = db_provider.clone();
                move |Json(payload): Json<RegisterAccount>| async move {
                    register_user_account(db_provider.clone(), payload).await
                }
            }),
        )
        .route(
            "/api/add_account",
            axum::routing::post({
                let db_provider = db_provider.clone();
                move |Json(payload): Json<AddAccount>| async move {
                    add_account(db_provider.clone(), payload).await
                }
            }),
        )
}

pub async fn status() -> impl IntoResponse {
    println!("Service is running...");
    let response = json!({
        "message": "Service is running...",
        "status": "ok"
    });
    (StatusCode::OK, Json(response))
}

pub async fn get_account<T: DBProvider + Send + Sync + Clone + 'static>(
    db_provider: Arc<T>,
    query: AddressQuery,
) -> impl IntoResponse {
    let account_service = AccountAggregationService::new(
        (*db_provider).clone(),
        "base_url".to_string(),
        "api_key".to_string(),
    );
    // match account_service.get_user_accounts(&query.address).await {
    //     Ok(accounts) => {
    //         let response = json!({ "accounts": accounts });
    //         (StatusCode::OK, Json(response))
    //     }
    //     Err(err) => {
    //         let response = json!({ "error": err.to_string() });
    //         (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
    //     }
    // }
    let accounts = account_service.get_user_accounts(&query.address).await;
    let response = json!({ "accounts": accounts });
    (StatusCode::OK, Json(response))
}

pub async fn register_user_account<T: DBProvider + Send + Sync + Clone + 'static>(
    db_provider: Arc<T>,
    payload: RegisterAccount,
) -> impl IntoResponse {
    let account_service = AccountAggregationService::new(
        (*db_provider).clone(),
        "base_url".to_string(),
        "api_key".to_string(),
    );
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

pub async fn add_account<T: DBProvider + Send + Sync + Clone + 'static>(
    db_provider: Arc<T>,
    payload: AddAccount,
) -> impl IntoResponse {
    let account_service = AccountAggregationService::new(
        (*db_provider).clone(),
        "base_url".to_string(),
        "api_key".to_string(),
    );
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
