use futures::stream::StreamExt;

use config::BucketConfig;
use storage;

use crate::{CostType, estimator, Route, RouteError, source, token_price};

const SOURCE_FETCH_PER_BUCKET_RATE_LIMIT: usize = 10;
const BUCKET_PROCESSING_RATE_LIMIT: usize = 5;

struct Indexer<
    'a,
    Source: source::RouteSource,
    ModelStore: storage::RoutingModelStore,
    Producer: storage::MessageQueue,
    TokenPriceProvider: token_price::TokenPriceProvider,
> {
    config: &'a config::Config,
    source: &'a Source,
    model_store: &'a ModelStore,
    message_producer: &'a Producer,
    token_price_provider: &'a TokenPriceProvider,
}

impl<
        'a,
        RouteSource: source::RouteSource,
        ModelStore: storage::RoutingModelStore,
        Producer: storage::MessageQueue,
        TokenPriceProvider: token_price::TokenPriceProvider,
    > Indexer<'a, RouteSource, ModelStore, Producer, TokenPriceProvider>
{
    fn new(
        config: &'a config::Config,
        source: &'a RouteSource,
        model_store: &'a ModelStore,
        message_producer: &'a Producer,
        token_price_provider: &'a TokenPriceProvider,
    ) -> Self {
        Indexer { config, source, model_store, message_producer, token_price_provider }
    }

    fn generate_bucket_observation_points(bucket: &BucketConfig) -> Vec<f64> {
        const POINTS_COUNT: u8 = 10;

        (0..=(POINTS_COUNT - 1))
            .into_iter()
            .map(|i| {
                bucket.token_amount_from_usd
                    + (i as f64) * (bucket.token_amount_to_usd - bucket.token_amount_from_usd)
                        / (POINTS_COUNT as f64)
            })
            .collect()
    }

    async fn build_estimator<Estimator: estimator::Estimator<'a, f64, f64>>(
        &self,
        bucket: &'a BucketConfig,
        cost_type: &CostType,
    ) -> Result<Estimator, Estimator::Error> {
        // Generate Data to "Train" Estimator
        let observation_points = Indexer::<RouteSource,ModelStore,Producer,TokenPriceProvider>::generate_bucket_observation_points(bucket);

        let data_points = futures::stream::iter(observation_points)
            .map(|input_value_in_usd: f64| {
                async move {
                    // Convert input_value_in_usd to token_amount_in_wei
                    let from_token_amount_in_wei =
                        token_price::utils::get_token_amount_from_value_in_usd(
                            &self.config,
                            self.token_price_provider,
                            &bucket.from_token,
                            bucket.from_chain_id,
                            input_value_in_usd,
                        )
                        .await
                        .map_err(|err| IndexerErrors::TokenPriceProviderError(err))?;

                    // Get the fee in usd from source
                    let route = Route::build(bucket, self.config)
                        .map_err(|err| IndexerErrors::RouteBuildError(err))?;
                    let fee_in_usd = self
                        .source
                        .fetch_least_route_cost_in_usd(&route, from_token_amount_in_wei, cost_type)
                        .await
                        .map_err(|err| IndexerErrors::RouteSourceError(err))?;

                    Ok::<
                        estimator::DataPoint<f64, f64>,
                        IndexerErrors<'a, TokenPriceProvider, RouteSource>,
                    >(estimator::DataPoint {
                        x: input_value_in_usd,
                        y: fee_in_usd,
                    })
                }
            })
            .buffer_unordered(SOURCE_FETCH_PER_BUCKET_RATE_LIMIT)
            .collect::<Vec<
                Result<
                    estimator::DataPoint<f64, f64>,
                    IndexerErrors<'a, TokenPriceProvider, RouteSource>,
                >,
            >>()
            .await
            .into_iter()
            .filter(|r| r.is_ok())
            .map(|r| r.unwrap())
            .collect();

        // Build the Estimator
        Estimator::build(data_points)
    }
}

#[derive(Debug)]
enum IndexerErrors<'a, T: token_price::TokenPriceProvider, S: source::RouteSource> {
    RouteBuildError(RouteError),
    TokenPriceProviderError(token_price::utils::Errors<'a, T::Error>),
    RouteSourceError(S::FetchRouteCostError),
}

#[cfg(test)]
mod tests {
    use std::env;

    use config::Config;
    use storage::{MessageQueue, RoutingModelStore};

    use crate::CostType;
    use crate::estimator::{Estimator, LinearRegressionEstimator};
    use crate::indexer::Indexer;
    use crate::source::BungeeClient;
    use crate::token_price::TokenPriceProvider;

    struct ModelStoreStub;
    impl RoutingModelStore for ModelStoreStub {
        type Error = ();

        async fn get(&self, k: String) -> Result<String, Self::Error> {
            Ok("Get".to_string())
        }

        async fn set(&self, k: String, v: String) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct ProducerStub;
    impl MessageQueue for ProducerStub {
        async fn publish(&self, topic: &str, message: &str) -> Result<(), String> {
            Ok(())
        }

        async fn subscribe(&self, topic: &str) -> Result<String, String> {
            Ok("Subscribed".to_string())
        }
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;
    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = String;

        async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error> {
            Ok(1.0) // USDC
        }
    }

    fn setup<'a>() -> (Config, BungeeClient, ModelStoreStub, ProducerStub, TokenPriceProviderStub) {
        // let config = config::Config::from_file("../../config.yaml").unwrap();
        let mut config = config::Config::from_yaml_str(
            r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
  - id: 42161
    name: Arbitrum
    is_enabled: true
tokens:
  - symbol: USDC
    is_enabled: true
    by_chain:
      1:
        is_enabled: true
        decimals: 6
        address: '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'
      42161:
        is_enabled: true
        decimals: 6
        address: '0xaf88d065e77c8cC2239327C5EDb3A432268e5831'
buckets:
  - from_chain_id: 1
    to_chain_id: 42161
    from_token: USDC
    to_token: USDC
    is_smart_contract_deposit_supported: false
    token_amount_from_usd: 1
    token_amount_to_usd: 10
  - from_chain_id: 1
    to_chain_id: 42161
    from_token: USDC
    to_token: USDC
    is_smart_contract_deposit_supported: false
    token_amount_from_usd: 10
    token_amount_to_usd: 100
  - from_chain_id: 1
    to_chain_id: 42161
    from_token: USDC
    to_token: USDC
    is_smart_contract_deposit_supported: false
    token_amount_from_usd: 100
    token_amount_to_usd: 1000
bungee:
  base_url: https://api.socket.tech/v2
  api_key: <REDACTED>
covalent:
  base_url: 'https://api.bungee.exchange'
  api_key: 'my-api'
coingecko:
  base_url: 'https://api.coingecko.com'
  api_key: 'my-api'
infra:
  redis_url: 'redis://localhost:6379'
  rabbitmq_url: 'amqp://localhost:5672'
  mongo_url: 'mongodb://localhost:27017'
server:
  port: 8080
  host: 'localhost'
is_indexer: true
        "#,
        )
        .unwrap();

        config.bungee.api_key = env::var("BUNGEE_API_KEY").unwrap();

        let bungee_client = BungeeClient::new(&config.bungee).unwrap();
        let model_store = ModelStoreStub;
        let message_producer = ProducerStub;
        let token_price_provider = TokenPriceProviderStub;

        return (config, bungee_client, model_store, message_producer, token_price_provider);
    }

    #[tokio::test]
    async fn test_build_estimator() {
        let (config, bungee_client, model_store, message_producer, token_price_provider) = setup();
        let indexer = Indexer::new(
            &config,
            &bungee_client,
            &model_store,
            &message_producer,
            &token_price_provider,
        );

        let estimator = indexer.build_estimator(&config.buckets[0], &CostType::Fee).await;
        assert!(estimator.is_ok());

        let estimator: LinearRegressionEstimator = estimator.unwrap();
        assert!(estimator.estimate(2.0) > 0.0);
    }
}
