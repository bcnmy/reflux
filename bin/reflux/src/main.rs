use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use axum::http::Method;
use clap::Parser;
use dotenv::dotenv;
use log::{debug, error, info};
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};

use account_aggregation::service::AccountAggregationService;
use api::service_controller::ServiceController;
use config::Config;
use routing_engine::{BungeeClient, CoingeckoClient, Indexer};
use routing_engine::estimator::LinearRegressionEstimator;
use routing_engine::routing_engine::RoutingEngine;
use routing_engine::settlement_engine::{generate_erc20_instance_map, SettlementEngine};
use storage::{ControlFlow, MessageQueue, RedisClient};
use storage::mongodb_client::MongoDBClient;

use crate::utils::find_lowest_transfer_amount_usd;

mod utils;

#[derive(Parser, Debug)]
struct Args {
    /// Run the Solver (default)
    #[arg(short, long)]
    solver: bool,

    /// Run the Indexer
    #[arg(short, long)]
    indexer: bool,

    /// Run the utility to find the lowest transfer amount in USD for all possible token pairs
    #[arg(short, long)]
    find_lowest_transfer_amounts_usd: bool,

    /// Config file path
    #[arg(short, long, default_value = "config.yaml")]
    config: String,
}

#[tokio::main]
async fn main() {
    if let Err(e) = dotenv() {
        error!("Failed to load .env file: {}", e);
    }
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let args = Args::parse();
    debug!("Args: {:?}", args);

    if args.indexer && args.solver {
        panic!("Cannot run both indexer and solver at the same time");
    }

    // Load configuration from yaml
    let config =
        Arc::new(Config::build_from_file(&args.config).expect("Failed to load config file"));

    if args.indexer {
        run_indexer(config).await;
    } else if args.solver {
        run_solver(config).await;
    } else if args.find_lowest_transfer_amounts_usd {
        run_find_lowest_transfer_amount_usd(config).await;
    } else {
        info!("Running Solver by default");
        run_solver(config).await;
    }
}

async fn run_solver(config: Arc<Config>) {
    info!("Starting Reflux Server");

    let (app_host, app_port) = (config.server.host.clone(), config.server.port.clone());

    // Instance of MongoDBProvider for users and account mappings
    let user_db_provider = MongoDBClient::new(
        &config.infra.mongo_url,
        "reflux".to_string(),
        "users".to_string(),
        true,
    )
    .await
    .expect("Failed to create MongoDB provider for users");
    let account_mapping_db_provider = MongoDBClient::new(
        &config.infra.mongo_url,
        "reflux".to_string(),
        "account_mappings".to_string(),
        false,
    )
    .await
    .expect("Failed to create MongoDB provider for account mappings");

    let (covalent_base_url, covalent_api_key) =
        (config.covalent.base_url.clone(), config.covalent.api_key.clone());

    let networks: Vec<String> =
        config.chains.iter().map(|(_, chain)| chain.covalent_name.clone()).collect();

    // Initialize account aggregation service for api
    let account_service = Arc::new(AccountAggregationService::new(
        user_db_provider.clone(),
        account_mapping_db_provider.clone(),
        networks,
        covalent_base_url,
        covalent_api_key,
    ));

    // Initialize routing engine
    let buckets = config.buckets.clone();
    let chain_configs = config.chains.clone();
    let token_configs = config.tokens.clone();
    let redis_client = RedisClient::build(&config.infra.redis_url)
        .await
        .expect("Failed to instantiate redis client");
    let routing_engine = Arc::new(RoutingEngine::new(
        account_service.clone(),
        buckets,
        redis_client.clone(),
        config.solver_config.clone(),
        chain_configs,
        token_configs,
    ));

    // Initialize Settlement Engine and Dependencies
    let erc20_instance_map = generate_erc20_instance_map(&config).unwrap();
    let bungee_client = BungeeClient::new(&config.bungee.base_url, &config.bungee.api_key)
        .expect("Failed to Instantiate Bungee Client");
    let token_price_provider = Arc::new(Mutex::new(CoingeckoClient::new(
        config.coingecko.base_url.clone(),
        config.coingecko.api_key.clone(),
        redis_client.clone(),
        Duration::from_secs(config.coingecko.expiry_sec),
    )));
    let settlement_engine = Arc::new(SettlementEngine::new(
        Arc::clone(&config),
        bungee_client,
        Arc::clone(&token_price_provider),
        erc20_instance_map,
    ));

    // Subscribe to cache update messages
    let cache_update_topic = config.indexer_config.indexer_update_topic.clone();
    let routing_engine_clone = Arc::clone(&routing_engine);

    tokio::task::spawn_blocking(move || {
        let redis_client = redis_client.clone();
        if let Err(e) = redis_client.subscribe(&cache_update_topic, move |_msg| {
            info!("Received cache update notification");
            let routing_engine_clone = Arc::clone(&routing_engine_clone);
            tokio::spawn(async move {
                routing_engine_clone.refresh_cache().await;
            });
            ControlFlow::<()>::Continue
        }) {
            error!("Failed to subscribe to cache update topic: {}", e);
        }
    });

    let token_chain_map: HashMap<String, HashMap<u32, bool>> = config
        .tokens
        .iter()
        .map(|(token, token_config)| {
            let chain_supported = token_config
                .by_chain
                .iter()
                .map(|(chain_id, chain_config)| (*chain_id, chain_config.is_enabled))
                .collect();
            (token.clone(), chain_supported)
        })
        .collect();

    // run the cache refresh once at the start
    routing_engine.refresh_cache().await;

    // API service controller
    let chain_supported: Vec<(u32, String)> =
        config.chains.iter().map(|(id, chain)| (*id, chain.name.clone())).collect();
    let token_supported: Vec<String> =
        config.tokens.iter().map(|(_, token_config)| token_config.symbol.clone()).collect();
    let service_controller = ServiceController::new(
        account_service,
        routing_engine,
        settlement_engine,
        token_chain_map,
        chain_supported,
        token_supported,
    );

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH])
        .allow_headers(Any);

    let app = service_controller.router().layer(cors);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", app_host, app_port))
        .await
        .expect("Failed to bind port");

    axum::serve(listener, app.into_make_service()).await.unwrap();

    info!("Server stopped.");
}

async fn run_indexer(config: Arc<Config>) {
    info!("Configuring Indexer");

    let config = config;

    let redis_provider = RedisClient::build(&config.infra.redis_url)
        .await
        .expect("Failed to instantiate redis client");

    let bungee_client = BungeeClient::new(&config.bungee.base_url, &config.bungee.api_key)
        .expect("Failed to Instantiate Bungee Client");

    let token_price_provider = Arc::new(Mutex::new(CoingeckoClient::new(
        config.coingecko.base_url.clone(),
        config.coingecko.api_key.clone(),
        redis_provider.clone(),
        Duration::from_secs(config.coingecko.expiry_sec),
    )));

    let indexer: Indexer<BungeeClient, RedisClient, RedisClient, CoingeckoClient<RedisClient>> =
        Indexer::new(
            config,
            bungee_client,
            redis_provider.clone(),
            redis_provider.clone(),
            token_price_provider,
        );

    match indexer.run::<LinearRegressionEstimator>().await {
        Ok(_) => info!("Indexer Job Completed"),
        Err(e) => error!("Indexer Job Failed: {}", e),
    };
}

async fn run_find_lowest_transfer_amount_usd(config: Arc<Config>) {
    let redis_client = RedisClient::build(&config.infra.redis_url).await.unwrap();
    let bungee_client = BungeeClient::new(&config.bungee.base_url, &config.bungee.api_key)
        .expect("Failed to Instantiate Bungee Client");
    let token_price_provider = Arc::new(Mutex::new(CoingeckoClient::new(
        config.coingecko.base_url.clone(),
        config.coingecko.api_key.clone(),
        redis_client.clone(),
        Duration::from_secs(config.coingecko.expiry_sec),
    )));

    let mut results = Vec::new();

    // Intentionally not running these concurrently to avoid rate limiting
    for (_, token_a) in config.tokens.iter() {
        for (chain_a, _) in token_a.by_chain.iter() {
            for (_, token_b) in config.tokens.iter() {
                for (chain_b, _) in token_b.by_chain.iter() {
                    if token_a.symbol != token_b.symbol || chain_a != chain_b {
                        results.push((
                            (
                                *chain_a,
                                *chain_b,
                                token_a.symbol.clone(),
                                token_b.symbol.clone(),
                                false,
                            ),
                            find_lowest_transfer_amount_usd(
                                &config,
                                *chain_a,
                                *chain_b,
                                &token_a.symbol,
                                &token_b.symbol,
                                false,
                                &bungee_client,
                                Arc::clone(&token_price_provider),
                            )
                            .await,
                        ))
                    }
                }
            }
        }
    }

    for result in results {
        let (route, amount_result) = result;

        match amount_result {
            Ok(amount) => {
                info!(
                    "Found lowest transfer amount in USD for token {} on chain {} to token {} on chain {}: {}",
                    route.2, route.0, route.3, route.1, amount
                );
            }
            Err(e) => {
                error!("Failed to find lowest transfer amount in USD: {}", e);
            }
        }
    }
}
