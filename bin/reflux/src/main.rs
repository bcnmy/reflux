use dotenv::dotenv;
use tokio;
use account_aggregation::AccountAggregationService;
use storage::StorageProvider;
use api::service_controller;
use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};
use log::info;
use tokio::signal;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let mongodb_uri = std::env::var("MONGODB_URI").expect("You must set the MONGODB_URI environment var!");
    let covalent_api_key = std::env::var("COVALENT_API_KEY").expect("You must set the COVALENT_API_KEY environment");
    // create mongodb client
    let client = mongodb::Client::with_uri_str(&mongodb_uri).await.expect("Failed to create mongodb client");
    let storage = StorageProvider::new(client, "reflux".to_string(), "users".to_string()).await.expect("Failed to create storage provider");

    let account_aggregation_service = AccountAggregationService::new(
        storage, covalent_api_key,
    );

    // start the axum server
    let app_host = std::env::var("APP_HOST").unwrap_or("0.0.0.0".to_string());
    let app_port = std::env::var("APP_PORT").unwrap_or("80".to_string());

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH]);

    let app = service_controller::router(
        account_aggregation_service
    ).layer(cors);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", app_host, app_port))
        .await
        .expect("Failed to bind port");
    axum::serve(listener, app.into_make_service()).with_graceful_shutdown(shutdown_signal()).await.unwrap();

    info!("Server stopped.");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("Unable to handle ctrl+c");
    };
    #[cfg(unix)]
        let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("signal received, starting graceful shutdown");
}
