use account_aggregation::service::AccountAggregationService;
use api::service_controller::ServiceController;
use axum::http::Method;
use config::Config;
use log::info;
use routing_engine::engine::RoutingEngine;
use storage::mongodb_provider::MongoDBProvider;
use tokio;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() {
    // Load configuration from yaml
    let config = Config::from_file("config.yaml").expect("Failed to load config file");
    let mongodb_uri = config.infra.mongo_url;
    let (app_host, app_port) = (config.server.host, config.server.port);

    // Instance of MongoDBProvider for users and account mappings
    let user_db_provider =
        MongoDBProvider::new(&mongodb_uri, "reflux".to_string(), "users".to_string(), true)
            .await
            .expect("Failed to create MongoDB provider for users");
    let account_mapping_db_provider = MongoDBProvider::new(
        &mongodb_uri,
        "reflux".to_string(),
        "account_mappings".to_string(),
        false,
    )
    .await
    .expect("Failed to create MongoDB provider for account mappings");

    let (covalent_base_url, covalent_api_key) = (config.covalent.base_url, config.covalent.api_key);

    // Initialize account aggregation service for api
    let account_service = AccountAggregationService::new(
        user_db_provider.clone(),
        account_mapping_db_provider.clone(),
        covalent_base_url,
        covalent_api_key,
    );

    // Initialize routing engine
    let routing_engine = RoutingEngine::new(account_service.clone());

    // API service controller
    let service_controller = ServiceController::new(account_service, routing_engine);

    let cors = CorsLayer::new().allow_origin(Any).allow_methods([
        Method::GET,
        Method::POST,
        Method::PATCH,
    ]);

    let app = service_controller.router().layer(cors);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", app_host, app_port))
        .await
        .expect("Failed to bind port");
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

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
