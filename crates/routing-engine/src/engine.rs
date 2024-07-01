use std::collections::HashMap;
use std::sync::Arc;

use derive_more::Display;
use futures::stream::{self, StreamExt};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

use account_aggregation::service::AccountAggregationService;
use account_aggregation::types::Balance;
use config::{config::BucketConfig, SolverConfig};
use storage::{RedisClient, RedisClientError};

use crate::estimator::{Estimator, LinearRegressionEstimator};

#[derive(Serialize, Deserialize, Debug, Display, PartialEq, Clone)]
#[display(
    "Route: from_chain: {}, to_chain: {}, from_token: {}, to_token: {}, amount in usd: {}",
    from_chain,
    to_chain,
    from_token,
    to_token,
    amount_in_usd
)]
pub struct Route {
    pub from_chain: u32,
    pub to_chain: u32,
    pub from_token: String,
    pub to_token: String,
    pub amount_in_usd: f64,
}

/// (from_chain, to_chain, from_token, to_token)
#[derive(Debug)]
struct PathQuery(u32, u32, String, String);

#[derive(Error, Debug)]
pub enum RoutingEngineError {
    #[error("Redis error: {0}")]
    RedisError(#[from] RedisClientError),

    #[error("Estimator error: {0}")]
    EstimatorError(#[from] serde_json::Error),

    #[error("Cache error: {0}")]
    CacheError(String),

    #[error("User balance fetch error: {0}")]
    UserBalanceFetchError(String),
}

/// Routing Engine
/// This struct is responsible for calculating the best cost path for a user
#[derive(Debug)]
pub struct RoutingEngine {
    buckets: Vec<BucketConfig>,
    aas_client: AccountAggregationService,
    cache: Arc<RwLock<HashMap<String, String>>>, // (hash(bucket), hash(estimator_value)
    redis_client: RedisClient,
    estimates: SolverConfig,
}

impl RoutingEngine {
    pub fn new(
        aas_client: AccountAggregationService,
        buckets: Vec<BucketConfig>,
        redis_client: RedisClient,
        solver_config: SolverConfig,
    ) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));

        Self { aas_client, cache, buckets, redis_client, estimates: solver_config }
    }

    pub async fn refresh_cache(&self) {
        match self.redis_client.get_all_key_values().await {
            Ok(kv_pairs) => {
                info!("Refreshing cache from Redis.");
                let mut cache = self.cache.write().await;
                cache.clear();
                for (key, value) in kv_pairs.iter() {
                    cache.insert(key.clone(), value.clone());
                }
                info!("Cache refreshed with latest data from Redis.");
                debug!("Cache: {:?}", cache);
            }
            Err(e) => {
                error!("Failed to refresh cache from Redis: {}", e);
            }
        }
    }

    /// Get the best cost path for a user
    /// This function will get the user balances from the aas and then calculate the best cost path for the user
    pub async fn get_best_cost_path(
        &self,
        account: &str,
        to_chain: u32,
        to_token: &str,
        to_value: f64,
    ) -> Result<Vec<Route>, RoutingEngineError> {
        debug!(
            "Getting best cost path for user: {}, to_chain: {}, to_token: {}, to_value: {}",
            account, to_chain, to_token, to_value
        );
        let user_balances = self.get_user_balance_from_agg_service(&account).await?;
        debug!("User balances: {:?}", user_balances);

        // todo: for account aggregation, transfer same chain same asset first
        let direct_assets: Vec<_> =
            user_balances.iter().filter(|balance| balance.token == to_token).collect();
        debug!("Direct assets: {:?}", direct_assets);

        // Sort direct assets by A^x / C^y, here x=2 and y=1
        let x = self.estimates.x_value;
        let y = self.estimates.y_value;
        let mut sorted_assets: Vec<(&Balance, f64)> = stream::iter(direct_assets.into_iter())
            .then(|balance| async move {
                let fee_cost = self
                    .get_cached_data(
                        balance.amount_in_usd,
                        PathQuery(
                            balance.chain_id,
                            to_chain,
                            balance.token.to_string(),
                            to_token.to_string(),
                        ),
                    )
                    .await
                    .unwrap_or_default();
                (balance, fee_cost)
            })
            .collect()
            .await;

        sorted_assets.sort_by(|a, b| {
            let cost_a = (a.0.amount.powf(x)) / (a.1.powf(y));
            let cost_b = (b.0.amount.powf(x)) / (b.1.powf(y));
            cost_a.partial_cmp(&cost_b).unwrap()
        });

        let mut total_cost = 0.0;
        let mut total_amount_needed = to_value;
        let mut selected_assets: Vec<Route> = Vec::new();

        for (balance, fee) in sorted_assets {
            if total_amount_needed <= 0.0 {
                break;
            }
            let amount_to_take = if balance.amount >= total_amount_needed {
                total_amount_needed
            } else {
                balance.amount
            };
            total_amount_needed -= amount_to_take;
            total_cost += fee;

            selected_assets.push(Route {
                from_chain: balance.chain_id,
                to_chain,
                from_token: balance.token.clone(),
                to_token: to_token.to_string(),
                amount_in_usd: amount_to_take,
            });
        }

        // Handle swap/bridge for remaining amount if needed (non direct assets)
        if total_amount_needed > 0.0 {
            let swap_assets: Vec<&Balance> =
                user_balances.iter().filter(|balance| balance.token != to_token).collect();
            let mut sorted_assets: Vec<(&Balance, f64)> = stream::iter(swap_assets.into_iter())
                .then(|balance| async move {
                    let fee_cost = self
                        .get_cached_data(
                            balance.amount_in_usd,
                            PathQuery(
                                balance.chain_id,
                                to_chain,
                                balance.token.clone(),
                                to_token.to_string(),
                            ),
                        )
                        .await
                        .unwrap_or_default();
                    (balance, fee_cost)
                })
                .collect()
                .await;

            sorted_assets.sort_by(|a, b| {
                let cost_a = (a.0.amount.powf(x)) / (a.1.powf(y));
                let cost_b = (b.0.amount.powf(x)) / (b.1.powf(y));
                cost_a.partial_cmp(&cost_b).unwrap()
            });

            for (balance, fee_cost) in sorted_assets {
                if total_amount_needed <= 0.0 {
                    break;
                }

                let amount_to_take = if balance.amount_in_usd >= total_amount_needed {
                    total_amount_needed
                } else {
                    balance.amount_in_usd
                };

                total_amount_needed -= amount_to_take;
                total_cost += fee_cost;

                selected_assets.push(Route {
                    from_chain: balance.chain_id,
                    to_chain,
                    from_token: balance.token.clone(),
                    to_token: to_token.to_string(),
                    amount_in_usd: amount_to_take,
                });
            }
        }

        debug!("Selected assets: {:?}", selected_assets);
        info!(
            "Total cost for user: {} on chain {} to token {} is {}",
            account, to_chain, to_token, total_cost
        );

        Ok(selected_assets)
    }

    async fn get_cached_data(
        &self,
        target_amount: f64,
        path: PathQuery,
    ) -> Result<f64, RoutingEngineError> {
        let mut buckets_array: Vec<BucketConfig> = self
            .buckets
            .clone()
            .into_iter()
            .filter(|bucket| {
                bucket.from_chain_id == path.0
                    && bucket.to_chain_id == path.1
                    && bucket.from_token == path.2
                    && bucket.to_token == path.3
            })
            .collect();
        buckets_array.sort();

        let bucket = buckets_array
            .iter()
            .find(|window| {
                target_amount >= window.token_amount_from_usd
                    && target_amount <= window.token_amount_to_usd
            })
            .ok_or_else(|| {
                RoutingEngineError::CacheError("No matching bucket found".to_string())
            })?;

        let key = bucket.get_hash().to_string();

        let cache = self.cache.read().await;
        let value = cache
            .get(&key)
            .ok_or_else(|| RoutingEngineError::CacheError("No cached value found".to_string()))?;
        let estimator: LinearRegressionEstimator = serde_json::from_str(value)?;

        Ok(estimator.estimate(target_amount))
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(
        &self,
        account: &str,
    ) -> Result<Vec<Balance>, RoutingEngineError> {
        // Note: aas should always return vec of balances
        self.aas_client
            .get_user_accounts_balance(&account.to_string())
            .await
            .map_err(|e| RoutingEngineError::UserBalanceFetchError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use account_aggregation::service::AccountAggregationService;
    use config::{BucketConfig, SolverConfig};
    use storage::mongodb_client::MongoDBClient;

    use crate::engine::PathQuery;
    use crate::estimator::Estimator;
    use crate::{
        engine::{RoutingEngine, RoutingEngineError},
        estimator::{DataPoint, LinearRegressionEstimator},
    };

    #[tokio::test]
    async fn test_get_cached_data() -> Result<(), RoutingEngineError> {
        // Create dummy buckets
        let buckets = vec![
            BucketConfig {
                from_chain_id: 1,
                to_chain_id: 2,
                from_token: "USDC".to_string(),
                to_token: "ETH".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 1.0,
                token_amount_to_usd: 10.0,
            },
            BucketConfig {
                from_chain_id: 1,
                to_chain_id: 2,
                from_token: "USDC".to_string(),
                to_token: "ETH".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 10.0,
                token_amount_to_usd: 100.0,
            },
        ];

        // Create a dummy estimator and serialize it
        let dummy_estimator = LinearRegressionEstimator::build(vec![
            DataPoint { x: 0.0, y: 0.0 },
            DataPoint { x: 1.0, y: 1.0 },
            DataPoint { x: 2.0, y: 2.0 },
        ])
        .unwrap();
        let serialized_estimator = serde_json::to_string(&dummy_estimator)?;

        // Create a cache with a dummy bucket
        let key = buckets[0].get_hash().to_string();
        let mut cache = HashMap::new();
        cache.insert(key, serialized_estimator);

        // Create RoutingEngine instance with dummy data
        let user_db_provider = MongoDBClient::new(
            "mongodb://localhost:27017",
            "test".to_string(),
            "test".to_string(),
            true,
        )
        .await
        .unwrap();

        let aas_client = AccountAggregationService::new(
            user_db_provider.clone(),
            user_db_provider.clone(),
            vec!["eth-mainnet".to_string()],
            "https://api.covalent.com".to_string(),
            "my-api".to_string(),
        );
        let redis_client =
            storage::RedisClient::build(&"redis://localhost:6379".to_string()).await.unwrap();
        let estimates = SolverConfig {
            x_value: 2.0,
            y_value: 1.0,
        };
        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
        };

        // Define the target amount and path query
        let target_amount = 5.0;
        let path_query = PathQuery(1, 2, "USDC".to_string(), "ETH".to_string());

        // Call get_cached_data and assert the result
        let result = routing_engine.get_cached_data(target_amount, path_query).await?;
        assert!(result > 0.0);
        assert_eq!(result, dummy_estimator.estimate(target_amount));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_best_cost_path() -> Result<(), RoutingEngineError> {
        let api_key = env::var("COVALENT_API_KEY");
        if api_key.is_err() {
            panic!("COVALENT_API_KEY is not set");
        }
        let api_key = api_key.unwrap();

        let user_db_provider = MongoDBClient::new(
            "mongodb://localhost:27017",
            "test".to_string(),
            "test".to_string(),
            true,
        )
        .await
        .unwrap();
        let aas_client = AccountAggregationService::new(
            user_db_provider.clone(),
            user_db_provider.clone(),
            vec!["bsc-mainnet".to_string()],
            "https://api.covalenthq.com".to_string(),
            api_key,
        );

        let buckets = vec![
            BucketConfig {
                from_chain_id: 56,
                to_chain_id: 2,
                from_token: "USDT".to_string(),
                to_token: "USDT".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 0.0,
                token_amount_to_usd: 5.0,
            },
            BucketConfig {
                from_chain_id: 56,
                to_chain_id: 2,
                from_token: "USDT".to_string(),
                to_token: "USDT".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 5.0,
                token_amount_to_usd: 100.0,
            },
        ];
        // Create a dummy estimator and serialize it
        let dummy_estimator = LinearRegressionEstimator::build(vec![
            DataPoint { x: 0.0, y: 0.0 },
            DataPoint { x: 1.0, y: 1.0 },
            DataPoint { x: 2.0, y: 2.0 },
        ])
        .unwrap();
        let serialized_estimator = serde_json::to_string(&dummy_estimator)?;
        // Create a cache with a dummy bucket
        let key1 = buckets[0].get_hash().to_string();
        let key2 = buckets[1].get_hash().to_string();
        let mut cache = HashMap::new();
        cache.insert(key1, serialized_estimator.clone());
        cache.insert(key2, serialized_estimator);

        let redis_client =
            storage::RedisClient::build(&"redis://localhost:6379".to_string()).await.unwrap();
            let estimates = SolverConfig {
                x_value: 2.0,
                y_value: 1.0,
            };
        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
        };

        // should have USDT in bsc-mainnet > $0.5
        let dummy_user_address = "0x00000ebe3fa7cb71aE471547C836E0cE0AE758c2";
        let result = routing_engine.get_best_cost_path(dummy_user_address, 2, "USDT", 0.5).await?;
        assert_eq!(result.len(), 1);
        Ok(())
    }
}
