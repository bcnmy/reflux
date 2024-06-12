use axum::{
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use account_aggregation::AccountAggregationService;

pub async fn get_all_users(account_aggregation_service: AccountAggregationService) -> impl IntoResponse {
    let response = json!({
        "message": format!("Hello"),
    });

    account_aggregation_service.get_user_accounts(&String::from("0x1234")).await;

    (StatusCode::OK, Json(response))
}

