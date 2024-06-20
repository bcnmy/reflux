use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use derive_more::Display;
use futures::stream::StreamExt;

use config::config::BucketConfig;

use crate::{CostType, estimator, Route, RouteError, source, token_price};

const SOURCE_FETCH_PER_BUCKET_RATE_LIMIT: usize = 10;
const BUCKET_PROCESSING_RATE_LIMIT: usize = 5;

pub struct Indexer<
    'config,
    Source: source::RouteSource,
    ModelStore: storage::KeyValueStore,
    Producer: storage::MessageQueue,
    TokenPriceProvider: token_price::TokenPriceProvider,
> {
    config: &'config config::Config,
    source: &'config Source,
    model_store: &'config ModelStore,
    message_producer: &'config Producer,
    token_price_provider: &'config TokenPriceProvider,
}

const POINTS_COUNT_PER_BUCKET: u8 = 10;

impl<
        'config,
        RouteSource: source::RouteSource,
        ModelStore: storage::KeyValueStore,
        Producer: storage::MessageQueue,
        TokenPriceProvider: token_price::TokenPriceProvider,
    > Indexer<'config, RouteSource, ModelStore, Producer, TokenPriceProvider>
{
    pub fn new(
        config: &'config config::Config,
        source: &'config RouteSource,
        model_store: &'config ModelStore,
        message_producer: &'config Producer,
        token_price_provider: &'config TokenPriceProvider,
    ) -> Self {
        Indexer { config, source, model_store, message_producer, token_price_provider }
    }

    fn generate_bucket_observation_points(bucket: &BucketConfig) -> Vec<f64> {
        (0..POINTS_COUNT_PER_BUCKET)
            .into_iter()
            .map(|i| {
                bucket.token_amount_from_usd
                    + (i as f64) * (bucket.token_amount_to_usd - bucket.token_amount_from_usd)
                        / (POINTS_COUNT_PER_BUCKET as f64)
            })
            .collect()
    }

    async fn build_estimator<'est_de, Estimator: estimator::Estimator<'est_de, f64, f64>>(
        &self,
        bucket: &'config BucketConfig,
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
                            &input_value_in_usd,
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
                        IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer>,
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
                    IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer>,
                >,
            >>()
            .await
            .into_iter()
            .filter(|r| r.is_ok())
            .map(|r| match r {
                Result::Ok(data_point) => data_point,
                _ => unreachable!(),
            })
            .collect();

        // Build the Estimator
        Estimator::build(data_points)
    }

    async fn publish_estimators<
        'est,
        'est_de,
        Estimator: estimator::Estimator<'est_de, f64, f64>,
    >(
        &self,
        values: Vec<(&&BucketConfig, &Estimator)>,
    ) -> Result<(), IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer>> {
        let values_transformed = values
            .iter()
            .map(|(k, v)| {
                let mut s = DefaultHasher::new();
                k.hash(&mut s);
                let key = s.finish().to_string();

                let value = serde_json::to_string(v).unwrap();

                (key, value)
            })
            .collect::<Vec<(String, String)>>();

        Ok(self
            .model_store
            .set_multiple(&values_transformed)
            .await
            .map_err(IndexerErrors::PublishEstimatorError)?)
    }

    pub async fn run<'est_de, Estimator: estimator::Estimator<'est_de, f64, f64>>(
        &self,
    ) -> Result<
        HashMap<&'config BucketConfig, Estimator>,
        IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer>,
    > {
        // Build Estimators
        let estimator_map: HashMap<&BucketConfig, Estimator> =
            futures::stream::iter(self.config.buckets.iter())
                .map(|bucket| async {
                    // Build the Estimator
                    let estimator: Estimator = self.build_estimator(bucket, &CostType::Fee).await?;

                    Ok::<(&BucketConfig, Estimator), Estimator::Error>((bucket, estimator))
                })
                .buffer_unordered(BUCKET_PROCESSING_RATE_LIMIT)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .filter(|r| r.is_ok())
                .map(|r| r.unwrap())
                .collect();

        self.publish_estimators(estimator_map.iter().collect()).await?;

        // Broadcast a Message to other nodes to update their cache
        self.message_producer
            .publish(
                &self.config.indexer_config.indexer_update_topic,
                &self.config.indexer_config.indexer_update_message,
            )
            .await
            .map_err(IndexerErrors::PublishIndexerUpdateMessageError)?;

        Ok(estimator_map)
    }
}

#[derive(Debug, Display)]
enum IndexerErrors<
    T: token_price::TokenPriceProvider,
    S: source::RouteSource,
    R: storage::KeyValueStore,
    U: storage::MessageQueue,
> {
    #[display("Route build error: {}", _0)]
    RouteBuildError(RouteError),

    #[display("Token price provider error: {}", _0)]
    TokenPriceProviderError(token_price::utils::Errors<T::Error>),

    #[display("Route source error: {}", _0)]
    RouteSourceError(S::FetchRouteCostError),

    #[display("Publish estimator error: {}", _0)]
    PublishEstimatorError(R::Error),

    #[display("Publish estimator errors: {:?}", _0)]
    PublishEstimatorErrors(Vec<R::Error>),

    #[display("Indexer update message error: {}", _0)]
    PublishIndexerUpdateMessageError(U::Error),
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fmt::Error;

    use derive_more::Display;
    use thiserror::Error;

    use config::Config;
    use storage::{ControlFlow, KeyValueStore, MessageQueue, Msg};

    use crate::CostType;
    use crate::estimator::{Estimator, LinearRegressionEstimator};
    use crate::indexer::Indexer;
    use crate::source::BungeeClient;
    use crate::token_price::TokenPriceProvider;

    #[derive(Error, Display, Debug)]
    struct Err;

    #[derive(Debug)]
    struct ModelStoreStub;
    impl KeyValueStore for ModelStoreStub {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            Ok("Get".to_string())
        }

        async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            Ok(vec!["Get".to_string(); k.len()])
        }

        async fn set(&self, k: &String, v: &String) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn set_multiple(&self, kv: &Vec<(String, String)>) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct ProducerStub;
    impl MessageQueue for ProducerStub {
        type Error = Err;

        async fn publish(&self, topic: &str, message: &str) -> Result<(), Self::Error> {
            Ok(())
        }

        fn subscribe<String>(
            &self,
            topic: &str,
            callback: impl FnMut(Msg) -> ControlFlow<String>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;
    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = Error;

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
    coingecko_symbol: usd-coin
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
  base_url: 'https://api.coingecko.com/api/v3'
  api_key: 'my-api'
infra:
  redis_url: 'redis://localhost:6379'
  rabbitmq_url: 'amqp://localhost:5672'
  mongo_url: 'mongodb://localhost:27017'
server:
  port: 8080
  host: 'localhost'
indexer_config:
    is_indexer: true
    indexer_update_topic: indexer_update
    indexer_update_message: message
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
        let (
            config,
            bungee_client,
            mut model_store,
            mut message_producer,
            mut token_price_provider,
        ) = setup();
        let mut indexer = Indexer::new(
            &config,
            &bungee_client,
            &mut model_store,
            &mut message_producer,
            &mut token_price_provider,
        );

        let estimator = indexer.build_estimator(&config.buckets[0], &CostType::Fee).await;
        assert!(estimator.is_ok());

        let estimator: LinearRegressionEstimator = estimator.unwrap();
        assert!(estimator.estimate(2.0) > 0.0);
    }
}
