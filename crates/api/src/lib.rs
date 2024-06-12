mod controller;

pub mod service_controller {
    use axum::response::IntoResponse;
    use axum::Json;
    use axum::Router;
    use axum::routing::get;
    use hyper::StatusCode;
    use serde_json::json;
    use account_aggregation::AccountAggregationService;
    use crate::controller::get_all_users;

    pub fn router(account_aggregation_service: AccountAggregationService) -> Router {
        Router::new()
            .route("/", get(status))
            .route("/api/health", get(status))
            .route("/api/get", get(|| async {
                get_all_users(account_aggregation_service).await
            }))
    }

    pub async fn status() -> impl IntoResponse {
        println!("Service is running...");
        let response = json!({
        "message": "Service is running...",
        "status": "ok"
    });
        (StatusCode::OK, Json(response))
    }
}
