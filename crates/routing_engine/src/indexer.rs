use derive_more::{Display, From};

use config::BucketConfig;
use storage;

use crate::{estimator, token_price};
use crate::source;

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

    async fn build_estimator<
        EstimatorError,
        Estimator: estimator::Estimator<'a, f64, f64, EstimatorError>,
    >(
        &self,
        bucket: &BucketConfig,
    ) -> Estimator {
        // Generate Data to "Train" Estimator
        let observation_points = Indexer::<RouteSource,ModelStore,Producer,TokenPriceProvider>::generate_bucket_observation_points(bucket);

        let fee_at_points =
            observation_points.into_iter().map(|input_value_in_usd: f64| async move {
                let token = &bucket.from_token;

                // Convert input_value_in_usd to token_amount_in_wei
            });

        // let estimator = Estimator::build()

        todo!()
    }
}

#[derive(Debug, Display, From)]
enum IndexerErrors<'a> {
    #[display(fmt = "Could not find token configuration for {}", _0)]
    TokenConfigurationNotFound(&'a String),

    #[display(fmt = "Could not find token configuration for {} on chain {}", _0, _1)]
    TokenConfigurationNotFoundForChain(&'a String, u32),
}
