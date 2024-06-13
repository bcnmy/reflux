use api::service_controller;
use axum::http::Method;
use config::Config;
use log::info;
use storage::mongodb_provider::MongoDBProvider;
use tokio;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() {
    let config = Config::from_file("config.yaml").expect("Failed to load config file");

    let mongodb_uri = config.infra.mongo_url;
    let (app_host, app_port) = (config.server.host, config.server.port);
    // create mongodb client
    let client =
        mongodb::Client::with_uri_str(&mongodb_uri).await.expect("Failed to create mongodb client");
    let db_provider = MongoDBProvider::new(client, "reflux".to_string(), "accounts".to_string())
        .await
        .expect("Failed to create MongoDB provider");

    let cors = CorsLayer::new().allow_origin(Any).allow_methods([
        Method::GET,
        Method::POST,
        Method::PATCH,
    ]);

    let app = service_controller::router(db_provider.clone()).layer(cors);

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
