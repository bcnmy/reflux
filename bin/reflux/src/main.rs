use std::sync::Arc;
use std::time::Duration;

use axum::http::Method;
use log::{error, info};
use tokio;
use tokio::signal;
use tokio_cron_scheduler::{Job, JobScheduler};
use tower_http::cors::{Any, CorsLayer};

use account_aggregation::service::AccountAggregationService;
use api::service_controller::ServiceController;
use config::Config;
use routing_engine::{BungeeClient, CoingeckoClient, Indexer};
use routing_engine::engine::RoutingEngine;
use routing_engine::estimator::LinearRegressionEstimator;
use storage::mongodb_provider::MongoDBProvider;
use storage::RedisClient;

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new().env().init().unwrap();

    // Load configuration from yaml
    let config = Config::from_file("config.yaml").expect("Failed to load config file");

    if config.indexer_config.is_indexer {
        run_indexer(config).await;
    } else {
        run_server(config).await;
    }
}

async fn run_server(config: Config) {
    info!("Starting Reflux Server");

    let (app_host, app_port) = (config.server.host.clone(), config.server.port.clone());

    // Instance of MongoDBProvider for users and account mappings
    let user_db_provider = MongoDBProvider::new(
        &config.infra.mongo_url,
        "reflux".to_string(),
        "users".to_string(),
        true,
    )
    .await
    .expect("Failed to create MongoDB provider for users");
    let account_mapping_db_provider = MongoDBProvider::new(
        &config.infra.mongo_url,
        "reflux".to_string(),
        "account_mappings".to_string(),
        false,
    )
    .await
    .expect("Failed to create MongoDB provider for account mappings");

    let (covalent_base_url, covalent_api_key) =
        (config.covalent.base_url.clone(), config.covalent.api_key.clone());

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

async fn run_indexer(config: Config) {
    info!("Configuring Indexer");

    let schedule = config.indexer_config.schedule.clone();
    let scheduler = JobScheduler::new().await.expect("Failed to create scheduler");

    let config = Arc::new(config);

    let job = Job::new_async(schedule.as_str(), move |uuid, mut l: JobScheduler| {
        /*
           TODO: I"m not sure why this works tbh
           Observations:
           1. If I don't use Arc, I get an error stating that the value is moved
           2. If this closure (not the async block) is not move, I get an error stating that the value is moved
           3. If I borrow config in this closure, I get a dangling reference error
           A combination of using Arc and move in the closure seems to work, but I'm not sure why
        */

        let config = Arc::clone(&config);

        Box::pin(async move {
            let redis_provider = RedisClient::build(&config.infra.redis_url)
                .await
                .expect("Failed to instantiate redis client");

            let bungee_client = BungeeClient::new(&config.bungee.base_url, &config.bungee.api_key)
                .expect("Failed to Instantiate Bungee Client");

            let token_price_provider = CoingeckoClient::new(
                &config.coingecko.base_url,
                &config.coingecko.api_key,
                &redis_provider,
                Duration::from_secs(config.coingecko.expiry_sec),
            );

            let indexer: Indexer<
                BungeeClient,
                RedisClient,
                RedisClient,
                CoingeckoClient<RedisClient>,
            > = Indexer::new(
                &config,
                &bungee_client,
                &redis_provider,
                &redis_provider,
                &token_price_provider,
            );

            match indexer.run::<LinearRegressionEstimator>().await {
                Ok(_) => info!("Indexer Job Completed"),
                Err(e) => error!("Indexer Job Failed: {}", e),
            };

            let next_tick = l.next_tick_for_job(uuid).await;
            match next_tick {
                Ok(Some(ts)) => println!("Next time for the job is {:?}", ts),
                _ => println!("Could not get next tick for the job"),
            };
        })
    })
    .expect("Failed to create job");

    scheduler.add(job).await.expect("Failed to add job");
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
