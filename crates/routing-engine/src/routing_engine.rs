use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use log::{debug, error, info};
use thiserror::Error;
use tokio::sync::RwLock;

use account_aggregation::service::AccountAggregationService;
use account_aggregation::types::TokenWithBalance;
use config::{ChainConfig, config::BucketConfig, SolverConfig, TokenConfig};
use storage::{KeyValueStore, RedisClient, RedisClientError};

use crate::{
    BridgeResult,
    estimator::{Estimator, LinearRegressionEstimator}, Route,
};

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
    chain_configs: HashMap<u32, ChainConfig>,
    token_configs: HashMap<String, TokenConfig>,
}

impl RoutingEngine {
    pub fn new(
        aas_client: AccountAggregationService,
        buckets: Vec<BucketConfig>,
        redis_client: RedisClient,
        solver_config: SolverConfig,
        chain_configs: HashMap<u32, ChainConfig>,
        token_configs: HashMap<String, TokenConfig>,
    ) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));

        Self {
            aas_client,
            cache,
            buckets,
            redis_client,
            estimates: solver_config,
            chain_configs,
            token_configs,
        }
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
    pub async fn get_best_cost_paths(
        &self,
        account: &str,
        to_chain: u32,
        to_token: &str,
        to_value: f64,
    ) -> Result<Vec<BridgeResult>, RoutingEngineError> {
        debug!(
            "Getting best cost path for user: {}, to_chain: {}, to_token: {}, to_value: {}",
            account, to_chain, to_token, to_value
        );
        let user_balances = self.get_user_balance_from_agg_service(&account).await?;
        debug!("User balances: {:?}", user_balances);

        // todo: for account aggregation, transfer same chain same asset first
        let (direct_assets, non_direct_assets): (Vec<_>, _) =
            user_balances.into_iter().partition(|balance| balance.token == to_token);
        debug!("Direct assets: {:?}", direct_assets);

        let (mut selected_routes, total_amount_needed, mut total_cost) =
            self.generate_optimal_routes(direct_assets, to_chain, to_token, to_value).await?;

        // Handle swap/bridge for remaining amount if needed (non-direct assets)
        if total_amount_needed > 0.0 {
            let (swap_routes, _, swap_total_cost) = self
                .generate_optimal_routes(non_direct_assets, to_chain, to_token, total_amount_needed)
                .await?;

            selected_routes.extend(swap_routes);
            total_cost += swap_total_cost;
        }

        debug!("Selected assets: {:?}", selected_routes);
        info!(
            "Total cost for user: {} on chain {} to token {} is {}",
            account, to_chain, to_token, total_cost
        );

        Ok(selected_routes)
    }

    async fn generate_optimal_routes(
        &self,
        assets: Vec<TokenWithBalance>,
        to_chain: u32,
        to_token: &str,
        to_value_usd: f64,
    ) -> Result<(Vec<BridgeResult>, f64, f64), RoutingEngineError> {
        // Sort direct assets by Balance^x / Fee_Cost^y, here x=2 and y=1
        let x = self.estimates.x_value;
        let y = self.estimates.y_value;
        let mut assets_sorted_by_bridging_cost: Vec<(TokenWithBalance, f64)> =
            stream::iter(assets.into_iter())
                .then(|balance| async move {
                    let fee_cost = self
                        .estimate_bridging_cost(
                            balance.amount_in_usd,
                            PathQuery(
                                balance.chain_id,
                                to_chain,
                                balance.token.to_string(),
                                to_token.to_string(),
                            ),
                        )
                        .await;
                    (balance, fee_cost)
                })
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .filter_map(|(balance, cost)| match cost {
                    Ok(cost) => Some((balance, cost)),
                    Err(e) => {
                        error!("Failed to estimate bridging cost for balance {:?}: {}", balance, e);
                        None
                    }
                })
                .collect();

        // Greedily select bridging routes that
        assets_sorted_by_bridging_cost.sort_by(|a, b| {
            let cost_a = (a.0.amount.powf(x)) / (a.1.powf(y));
            let cost_b = (b.0.amount.powf(x)) / (b.1.powf(y));
            cost_a.partial_cmp(&cost_b).unwrap()
        });

        let mut total_cost = 0.0;
        let mut total_amount_needed = to_value_usd;
        let mut selected_routes: Vec<BridgeResult> = Vec::new();

        for (balance, fee) in assets_sorted_by_bridging_cost {
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

            selected_routes.push(self.build_bridging_route(
                balance.chain_id,
                to_chain,
                &balance.token,
                to_token,
                amount_to_take,
                false,
            )?);
        }

        Ok((selected_routes, total_amount_needed, total_cost))
    }

    async fn estimate_bridging_cost(
        &self,
        target_amount_in_usd: f64,
        path: PathQuery,
    ) -> Result<f64, RoutingEngineError> {
        // TODO: Maintain sorted list cache in cache, binary search
        let bucket = self
            .buckets
            .iter()
            .find(|&bucket| {
                let matches_path = bucket.from_chain_id == path.0
                    && bucket.to_chain_id == path.1
                    && bucket.from_token == path.2
                    && bucket.to_token == path.3;

                let matches_amount = target_amount_in_usd >= bucket.token_amount_from_usd
                    && target_amount_in_usd <= bucket.token_amount_to_usd;

                matches_path && matches_amount
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

        Ok(estimator.estimate(target_amount_in_usd))
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(
        &self,
        account: &str,
    ) -> Result<Vec<TokenWithBalance>, RoutingEngineError> {
        let balance = self
            .aas_client
            .get_user_accounts_balance(&account.to_string())
            .await
            .map_err(|e| RoutingEngineError::UserBalanceFetchError(e.to_string()))?;

        let balance = balance
            .into_iter()
            .filter(|balance| {
                self.chain_configs.contains_key(&balance.chain_id)
                    && self.token_configs.contains_key(&balance.token)
            })
            .collect();

        debug!("User balance: {:?}", balance);
        Ok(balance)
    }

    fn build_bridging_route(
        &self,
        from_chain_id: u32,
        to_chain_id: u32,
        from_token_id: &str,
        to_token_id: &str,
        token_amount_in_usd: f64,
        is_smart_contract_deposit: bool,
    ) -> Result<BridgeResult, RoutingEngineError> {
        let from_chain = self.chain_configs.get(&from_chain_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!(
                "Chain config not found for ID {}",
                from_chain_id
            ))
        })?;
        let to_chain = self.chain_configs.get(&to_chain_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Chain config not found for ID {}", to_chain_id))
        })?;
        let from_token = self.token_configs.get(from_token_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Token config not found for {}", from_token_id))
        })?;
        let to_token = self.token_configs.get(to_token_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Token config not found for {}", to_token_id))
        })?;

        Ok(BridgeResult {
            route: Route { from_chain, to_chain, from_token, to_token, is_smart_contract_deposit },
            source_amount_in_usd: token_amount_in_usd,
            from_address: "".to_string(), //  TODO
            to_address: "".to_string(),   //  TODO
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use account_aggregation::service::AccountAggregationService;
    use config::{BucketConfig, ChainConfig, SolverConfig, TokenConfig, TokenConfigByChainConfigs};
    use storage::mongodb_client::MongoDBClient;

    use crate::{
        estimator::{DataPoint, LinearRegressionEstimator},
        routing_engine::{RoutingEngine, RoutingEngineError},
    };
    use crate::estimator::Estimator;
    use crate::routing_engine::PathQuery;

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
        let estimates = SolverConfig { x_value: 2.0, y_value: 1.0 };
        let chain_configs = HashMap::new();
        let token_configs = HashMap::new();
        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
            chain_configs,
            token_configs,
        };

        // Define the target amount and path query
        let target_amount = 5.0;
        let path_query = PathQuery(1, 2, "USDC".to_string(), "ETH".to_string());

        // Call get_cached_data and assert the result
        let result = routing_engine.estimate_bridging_cost(target_amount, path_query).await?;
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
        let estimates = SolverConfig { x_value: 2.0, y_value: 1.0 };
        let chain_config1 = ChainConfig {
            id: 56,
            name: "bsc-mainnet".to_string(),
            is_enabled: true,
            covalent_name: "bsc-mainnet".to_string(),
        };
        let chain_config2 = ChainConfig {
            id: 2,
            name: "eth-mainnet".to_string(),
            is_enabled: true,
            covalent_name: "ethereum".to_string(),
        };
        let mut chain_configs = HashMap::new();
        chain_configs.insert(56, chain_config1);
        chain_configs.insert(2, chain_config2);

        let token_config = TokenConfig {
            symbol: "USDT".to_string(),
            coingecko_symbol: "USDT".to_string(),
            is_enabled: true,
            by_chain: TokenConfigByChainConfigs(HashMap::new()),
        };
        let mut token_configs = HashMap::new();
        token_configs.insert("USDT".to_string(), token_config);

        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
            chain_configs,
            token_configs,
        };

        // should have USDT in bsc-mainnet > $0.5
        let dummy_user_address = "0x00000ebe3fa7cb71aE471547C836E0cE0AE758c2";
        let result = routing_engine.get_best_cost_paths(dummy_user_address, 2, "USDT", 0.5).await?;
        assert_eq!(result.len(), 1);
        Ok(())
    }
}
