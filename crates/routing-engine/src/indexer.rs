use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use futures::stream::StreamExt;
use log::{error, info};
use thiserror::Error;

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

    fn generate_bucket_observation_points(&self, bucket: &BucketConfig) -> Vec<f64> {
        let points_per_bucket = self.config.indexer_config.points_per_bucket;
        (0..points_per_bucket)
            .into_iter()
            .map(|i| {
                bucket.token_amount_from_usd
                    + (i as f64) * (bucket.token_amount_to_usd - bucket.token_amount_from_usd)
                        / (points_per_bucket as f64)
            })
            .collect()
    }

    async fn build_estimator<'est_de, Estimator: estimator::Estimator<'est_de, f64, f64>>(
        &self,
        bucket: &'config BucketConfig,
        cost_type: &CostType,
    ) -> Result<Estimator, BuildEstimatorError<'config, 'est_de, Estimator>> {
        info!("Building estimator for bucket: {:?}", bucket);

        // Generate Data to "Train" Estimator
        let observation_points = self.generate_bucket_observation_points(bucket);
        info!("{} Observation points generated", observation_points.len());

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
                        IndexerErrors<
                            TokenPriceProvider,
                            RouteSource,
                            ModelStore,
                            Producer,
                            Estimator,
                        >,
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
                    IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer, Estimator>,
                >,
            >>()
            .await;

        let (data_points, failed): (Vec<Result<_, _>>, Vec<Result<_, _>>) =
            data_points.into_iter().partition(|r| r.is_ok());

        let data_points: Vec<estimator::DataPoint<f64, f64>> =
            data_points.into_iter().map(|r| r.unwrap()).collect();
        let failed: Vec<
            IndexerErrors<TokenPriceProvider, RouteSource, ModelStore, Producer, Estimator>,
        > = failed.into_iter().map(|r| r.unwrap_err()).collect();

        if failed.len() > 0 {
            error!("Failed to fetch some data points: {:?}", failed);
        }

        if data_points.is_empty() {
            error!("No data points remain for bucket: {:?}", bucket);
            return Err(BuildEstimatorError::NoDataPoints(bucket));
        }

        // Build the Estimator
        info!("All data points fetched, building estimator for bucket: {:?}", bucket);
        let estimator = Estimator::build(data_points)
            .map_err(|e| BuildEstimatorError::EstimatorBuildError(bucket, e))?;
        Ok(estimator)
    }

    async fn publish_estimators<
        'est,
        'est_de,
        Estimator: estimator::Estimator<'est_de, f64, f64>,
    >(
        &self,
        values: Vec<(&&BucketConfig, &Estimator)>,
    ) -> Result<
        (),
        IndexerErrors<'est_de, TokenPriceProvider, RouteSource, ModelStore, Producer, Estimator>,
    > {
        info!("Publishing {} estimators", values.len());

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
        IndexerErrors<'est_de, TokenPriceProvider, RouteSource, ModelStore, Producer, Estimator>,
    > {
        info!("Running Indexer");

        // Build Estimators
        let (estimators, failed_estimators): (Vec<_>, Vec<_>) = futures::stream::iter(
            self.config.buckets.iter(),
        )
        .map(|bucket: &_| async {
            // Build the Estimator
            let estimator = self.build_estimator(bucket, &CostType::Fee).await?;

            Ok::<(&BucketConfig, Estimator), BuildEstimatorError<'config, 'est_de, Estimator>>((
                bucket, estimator,
            ))
        })
        .buffer_unordered(BUCKET_PROCESSING_RATE_LIMIT)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .partition(|r| r.is_ok());

        let estimator_map: HashMap<&BucketConfig, Estimator> =
            estimators.into_iter().map(|r| r.unwrap()).collect();

        if !failed_estimators.is_empty() {
            error!("Failed to build some estimators: {:?}", failed_estimators);
        }

        if estimator_map.is_empty() {
            error!("No estimators built");
            return Err(IndexerErrors::NoEstimatorsBuilt);
        }

        self.publish_estimators(estimator_map.iter().collect()).await?;

        // Broadcast a Message to other nodes to update their cache
        info!("Broadcasting Indexer Update Message");
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

#[derive(Debug, Error)]
pub enum IndexerErrors<
    'a,
    T: token_price::TokenPriceProvider,
    S: source::RouteSource,
    R: storage::KeyValueStore,
    U: storage::MessageQueue,
    V: estimator::Estimator<'a, f64, f64>,
> {
    #[error("Route build error: {}", _0)]
    RouteBuildError(RouteError),

    #[error("Token price provider error: {}", _0)]
    TokenPriceProviderError(token_price::utils::Errors<T::Error>),

    #[error("Route source error: {}", _0)]
    RouteSourceError(S::FetchRouteCostError),

    #[error("Publish estimator error: {}", _0)]
    PublishEstimatorError(R::Error),

    #[error("Publish estimator errors: {:?}", _0)]
    PublishEstimatorErrors(Vec<R::Error>),

    #[error("Indexer update message error: {}", _0)]
    PublishIndexerUpdateMessageError(U::Error),

    #[error("Estimator build error: {}", _0)]
    EstimatorBuildError(V::Error),

    #[error("No estimators built")]
    NoEstimatorsBuilt,
}

#[derive(Debug, Error)]
pub enum BuildEstimatorError<'config, 'est_de, Estimator: estimator::Estimator<'est_de, f64, f64>> {
    #[error("No data points found while building estimator for {:?}", _0)]
    NoDataPoints(&'config BucketConfig),

    #[error("Estimator build error: {} for bucket {:?}", _1, _0)]
    EstimatorBuildError(&'config BucketConfig, Estimator::Error),
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fmt::Error;
    use std::time::Duration;

    use derive_more::Display;
    use thiserror::Error;

    use config::{Config, get_sample_config};
    use storage::{ControlFlow, KeyValueStore, MessageQueue, Msg};

    use crate::{BungeeClient, CostType};
    use crate::estimator::{Estimator, LinearRegressionEstimator};
    use crate::indexer::Indexer;
    use crate::token_price::TokenPriceProvider;

    #[derive(Error, Display, Debug)]
    struct Err;

    #[derive(Debug)]
    struct ModelStoreStub;
    impl KeyValueStore for ModelStoreStub {
        type Error = Err;

        async fn get(&self, _: &String) -> Result<String, Self::Error> {
            Ok("Get".to_string())
        }

        async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            Ok(vec!["Get".to_string(); k.len()])
        }

        async fn set(&self, _: &String, _: &String, _: Duration) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn set_multiple(&self, _: &Vec<(String, String)>) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct ProducerStub;
    impl MessageQueue for ProducerStub {
        type Error = Err;

        async fn publish(&self, _: &str, _: &str) -> Result<(), Self::Error> {
            Ok(())
        }

        fn subscribe<String>(
            &self,
            _: &str,
            _: impl FnMut(Msg) -> ControlFlow<String>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;
    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = Error;

        async fn get_token_price(&self, _token_symbol: &String) -> Result<f64, Self::Error> {
            Ok(1.0) // USDC
        }
    }

    fn setup<'a>() -> (Config, BungeeClient, ModelStoreStub, ProducerStub, TokenPriceProviderStub) {
        let mut config = get_sample_config();
        config.buckets = vec![
            config::BucketConfig {
                from_chain_id: 1,
                to_chain_id: 42161,
                from_token: "USDC".to_string(),
                to_token: "USDC".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 10.0,
                token_amount_to_usd: 100.0,
            },
            config::BucketConfig {
                from_chain_id: 1,
                to_chain_id: 42161,
                from_token: "USDC".to_string(),
                to_token: "USDC".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 100.0,
                token_amount_to_usd: 1000.0,
            },
        ];

        config.bungee.api_key = env::var("BUNGEE_API_KEY").unwrap();

        let bungee_client =
            BungeeClient::new(&config.bungee.base_url, &config.bungee.api_key).unwrap();
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
        let indexer = Indexer::new(
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
