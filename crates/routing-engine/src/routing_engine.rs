use std::cmp;
use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use log::{debug, error, info};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use account_aggregation::{service::AccountAggregationService, types::TokenWithBalance};
use config::{config::BucketConfig, ChainConfig, Config, SolverConfig, TokenConfig};
use storage::{KeyValueStore, RedisClient, RedisClientError};

use crate::token_price::utils::{get_token_price, Errors};
use crate::token_price::TokenPriceProvider;
use crate::{
    estimator::{Estimator, LinearRegressionEstimator},
    BridgeResult, BridgeResultVecWrapper, Route,
};

const FETCH_REDIS_KEYS_BATCH_SIZE: usize = 50;

/// (from_chain, to_chain, from_token, to_token)
#[derive(Debug)]
struct PathQuery(u32, u32, String, String);

#[derive(Error, Debug)]
pub enum RoutingEngineError<T: TokenPriceProvider> {
    #[error("Redis error: {0}")]
    RedisError(#[from] RedisClientError),

    #[error("Estimator error: {0}")]
    EstimatorError(#[from] serde_json::Error),

    #[error("Cache error: {0}")]
    CacheError(String),

    #[error("Bucket not found error: chain {0} -> {1}, token: {2} -> {3}, amount: {4}")]
    BucketNotFoundError(u32, u32, String, String, f64),

    #[error("User balance fetch error: {0}")]
    UserBalanceFetchError(String),

    #[error("Token price provider error: {0}")]
    TokenPriceProviderError(Errors<T::Error>),
}

/// Routing Engine
/// This struct is responsible for calculating the best cost path for a user
#[derive(Debug)]
pub struct RoutingEngine<T: TokenPriceProvider> {
    buckets: Vec<Arc<BucketConfig>>,
    aas_client: Arc<AccountAggregationService>,
    cache: Arc<RwLock<HashMap<String, String>>>, // (hash(bucket), hash(estimator_value)
    redis_client: RedisClient,
    estimates: Arc<SolverConfig>,
    chain_configs: HashMap<u32, Arc<ChainConfig>>,
    token_configs: HashMap<String, Arc<TokenConfig>>,
    config: Arc<Config>,
    price_provider: Arc<Mutex<T>>,
}

impl<PriceProvider: TokenPriceProvider> RoutingEngine<PriceProvider> {
    pub fn new(
        aas_client: Arc<AccountAggregationService>,
        buckets: Vec<Arc<BucketConfig>>,
        redis_client: RedisClient,
        solver_config: Arc<SolverConfig>,
        chain_configs: HashMap<u32, Arc<ChainConfig>>,
        token_configs: HashMap<String, Arc<TokenConfig>>,
        config: Arc<Config>,
        price_provider: Arc<Mutex<PriceProvider>>,
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
            config,
            price_provider,
        }
    }

    /// Refresh the cache from Redis
    pub async fn refresh_cache(&self) {
        match self.redis_client.get_all_key_values(Some(FETCH_REDIS_KEYS_BATCH_SIZE)).await {
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

    /// Get the best cost path for a user.
    /// This function will get the user balances from the aas and then calculate the best cost path for the user
    pub async fn get_best_cost_paths(
        &self,
        account: &str,
        to_chain: u32,
        to_token: &str,
        to_amount_token: f64,
    ) -> Result<Vec<BridgeResult>, RoutingEngineError<PriceProvider>> {
        debug!(
            "Getting best cost path for user: {}, to_chain: {}, to_token: {}, to_amount_token: {}",
            account, to_chain, to_token, to_amount_token
        );
        let user_balances = self.get_user_balance_from_agg_service(&account).await?;
        debug!("User balances: {:?}", user_balances);

        // todo: for account aggregation, transfer same chain same asset first
        let (direct_assets, non_direct_assets): (Vec<_>, _) =
            user_balances.into_iter().partition(|balance| balance.token == to_token);
        debug!("Direct assets: {:?}", direct_assets);
        debug!("Non-direct assets: {:?}", non_direct_assets);

        let to_value_usd =
            get_token_price(&self.config, &self.price_provider.lock().await, &to_token.to_string())
                .await
                .map_err(RoutingEngineError::TokenPriceProviderError)?
                * to_amount_token;
        debug!("To value in USD: {}", to_value_usd);

        let (mut selected_routes, total_amount_needed, mut total_cost) = self
            .generate_optimal_routes(direct_assets, to_chain, to_token, to_value_usd, account)
            .await?;

        // Handle swap/bridge for remaining amount if needed (non-direct assets)
        if total_amount_needed > 0.0 {
            let (swap_routes, _, swap_total_cost) = self
                .generate_optimal_routes(
                    non_direct_assets,
                    to_chain,
                    to_token,
                    total_amount_needed,
                    account,
                )
                .await?;

            selected_routes.extend(swap_routes);
            total_cost += swap_total_cost;
        }

        debug!("Selected assets: {}", BridgeResultVecWrapper(&selected_routes));
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
        to_address: &str,
    ) -> Result<(Vec<BridgeResult>, f64, f64), RoutingEngineError<PriceProvider>> {
        // Sort direct assets by Balance^x / Fee_Cost^y, here x=2 and y=1
        let x = self.estimates.x_value;
        let y = self.estimates.y_value;
        let mut assets_sorted_by_bridging_cost: Vec<(TokenWithBalance, f64)> =
            stream::iter(assets.into_iter())
                .then(|mut balance| async move {
                    let balance_taken = cmp::min_by(to_value_usd, balance.amount_in_usd, |a, b| {
                        a.partial_cmp(b).unwrap_or_else(|| cmp::Ordering::Less)
                    });
                    let fee_cost = self
                        .estimate_bridging_cost(
                            balance_taken,
                            PathQuery(
                                balance.chain_id,
                                to_chain,
                                balance.token.to_string(),
                                to_token.to_string(),
                            ),
                        )
                        .await;

                    if balance.token == "ETH" {
                        balance.amount_in_usd -= 1.0;
                    }
                    if balance.amount_in_usd < 0.0 {
                        balance.amount_in_usd = 0.0;
                    }
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
            let amount_to_take = if balance.amount_in_usd >= total_amount_needed {
                total_amount_needed
            } else {
                balance.amount_in_usd
            };
            total_amount_needed -= amount_to_take;
            total_cost += fee;

            selected_routes.push(self.build_bridging_route(
                balance.chain_id,
                to_chain,
                &balance.token,
                to_token,
                balance.amount,
                amount_to_take,
                false,
                &balance.address,
                &to_address,
            )?);
        }

        Ok((selected_routes, total_amount_needed, total_cost))
    }

    async fn estimate_bridging_cost(
        &self,
        target_amount_in_usd: f64,
        path: PathQuery,
    ) -> Result<f64, RoutingEngineError<PriceProvider>> {
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
                RoutingEngineError::BucketNotFoundError(
                    path.0,
                    path.1,
                    path.2.clone(),
                    path.3.clone(),
                    target_amount_in_usd,
                )
            })?;

        let key = bucket.get_hash().to_string();

        let cache = self.cache.read().await;
        let value = cache.get(&key).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("No cached value found for {}", key))
        })?;
        let estimator: LinearRegressionEstimator = serde_json::from_str(value)?;

        Ok(estimator.estimate(target_amount_in_usd))
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(
        &self,
        account: &str,
    ) -> Result<Vec<TokenWithBalance>, RoutingEngineError<PriceProvider>> {
        let balance = self
            .aas_client
            .get_user_accounts_balance(&account.to_string())
            .await
            .map_err(|e| RoutingEngineError::UserBalanceFetchError(e.to_string()))?;

        let balance: Vec<_> = balance
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
        token_amount: f64,
        token_amount_in_usd: f64,
        is_smart_contract_deposit: bool,
        from_address: &str,
        to_address: &str,
    ) -> Result<BridgeResult, RoutingEngineError<PriceProvider>> {
        let from_chain = Arc::clone(self.chain_configs.get(&from_chain_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!(
                "Chain config not found for ID {}",
                from_chain_id
            ))
        })?);
        let to_chain = Arc::clone(self.chain_configs.get(&to_chain_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Chain config not found for ID {}", to_chain_id))
        })?);
        let from_token = Arc::clone(self.token_configs.get(from_token_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Token config not found for {}", from_token_id))
        })?);
        let to_token = Arc::clone(self.token_configs.get(to_token_id).ok_or_else(|| {
            RoutingEngineError::CacheError(format!("Token config not found for {}", to_token_id))
        })?);

        Ok(BridgeResult {
            route: Route { from_chain, to_chain, from_token, to_token, is_smart_contract_deposit },
            source_amount: token_amount,
            source_amount_in_usd: token_amount_in_usd,
            from_address: from_address.to_string(),
            to_address: to_address.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::fmt::Error;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use derive_more::Display;
    use thiserror::Error;
    use tokio::sync::{Mutex, RwLock};

    use account_aggregation::service::AccountAggregationService;
    use config::{
        get_sample_config, BucketConfig, ChainConfig, SolverConfig, TokenConfig,
        TokenConfigByChainConfigs,
    };
    use storage::mongodb_client::MongoDBClient;
    use storage::{KeyValueStore, RedisClientError};

    use crate::estimator::Estimator;
    use crate::routing_engine::PathQuery;
    use crate::token_price::TokenPriceProvider;
    use crate::{
        estimator::{DataPoint, LinearRegressionEstimator},
        routing_engine::{RoutingEngine, RoutingEngineError},
        CoingeckoClient,
    };

    #[derive(Error, Debug, Display)]
    struct Err;

    #[derive(Default, Debug)]
    struct KVStore {
        map: Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl KeyValueStore for KVStore {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            match self.map.lock().await.get(k) {
                Some(v) => Ok(v.clone()),
                None => Result::Err(Err),
            }
        }

        async fn get_multiple(&self, _: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            unimplemented!()
        }

        async fn set(&self, k: &String, v: &String, _: Duration) -> Result<(), Self::Error> {
            self.map
                .lock()
                .await
                .insert((*k.clone()).parse().unwrap(), (*v.clone()).parse().unwrap());
            Ok(())
        }

        async fn set_multiple(&self, _: &Vec<(String, String)>) -> Result<(), Self::Error> {
            unimplemented!()
        }

        async fn get_all_keys(&self) -> Result<Vec<String>, RedisClientError> {
            unimplemented!()
        }

        async fn get_all_key_values(
            &self,
            _: Option<usize>,
        ) -> Result<HashMap<String, String>, RedisClientError> {
            unimplemented!()
        }
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;

    #[async_trait]
    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = Error;

        async fn get_token_price(&self, _: &String) -> Result<f64, Self::Error> {
            Ok(0.1)
        }
    }

    #[tokio::test]
    async fn test_get_cached_data() -> Result<(), RoutingEngineError<TokenPriceProviderStub>> {
        // Create dummy buckets
        let buckets = vec![
            Arc::new(BucketConfig {
                from_chain_id: 1,
                to_chain_id: 2,
                from_token: "USDC".to_string(),
                to_token: "ETH".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 1.0,
                token_amount_to_usd: 10.0,
            }),
            Arc::new(BucketConfig {
                from_chain_id: 1,
                to_chain_id: 2,
                from_token: "USDC".to_string(),
                to_token: "ETH".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 10.0,
                token_amount_to_usd: 100.0,
            }),
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

        let aas_client = Arc::new(AccountAggregationService::new(
            user_db_provider.clone(),
            user_db_provider.clone(),
            vec!["eth-mainnet".to_string()],
            "https://api.covalent.com".to_string(),
            "my-api".to_string(),
        ));
        let redis_client =
            storage::RedisClient::build(&"redis://localhost:6379".to_string()).await.unwrap();
        let estimates = Arc::new(SolverConfig { x_value: 2.0, y_value: 1.0 });
        let chain_configs = HashMap::new();
        let token_configs = HashMap::new();

        let api_key = env::var("COINGECKO_API_KEY").expect("COINGECKO_API_KEY is not set");

        let store = KVStore::default();

        let client = CoingeckoClient::new(
            "https://api.coingecko.com/api/v3".to_string(),
            api_key,
            store,
            Duration::from_secs(300),
        );

        let config = get_sample_config();

        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
            chain_configs,
            token_configs,
            config: Arc::new(config),
            price_provider: Arc::new(Mutex::new(client)),
        };

        // Define the target amount and path query
        let target_amount = 5.0;
        let path_query = PathQuery(1, 2, "USDC".to_string(), "ETH".to_string());

        // Call get_cached_data and assert the result
        let result = routing_engine
            .estimate_bridging_cost(target_amount, path_query)
            .await
            .expect("Failed to get cached data");
        assert!(result > 0.0);
        assert_eq!(result, dummy_estimator.estimate(target_amount));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_best_cost_path() -> Result<(), RoutingEngineError<TokenPriceProviderStub>> {
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
        let aas_client = Arc::new(AccountAggregationService::new(
            user_db_provider.clone(),
            user_db_provider.clone(),
            vec!["bsc-mainnet".to_string()],
            "https://api.covalenthq.com".to_string(),
            api_key,
        ));

        let buckets = vec![
            Arc::new(BucketConfig {
                from_chain_id: 56,
                to_chain_id: 2,
                from_token: "USDT".to_string(),
                to_token: "USDT".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 0.0,
                token_amount_to_usd: 5.0,
            }),
            Arc::new(BucketConfig {
                from_chain_id: 56,
                to_chain_id: 2,
                from_token: "USDT".to_string(),
                to_token: "USDT".to_string(),
                is_smart_contract_deposit_supported: false,
                token_amount_from_usd: 5.0,
                token_amount_to_usd: 100.0,
            }),
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
        let estimates = Arc::new(SolverConfig { x_value: 2.0, y_value: 1.0 });
        let chain_config1 = Arc::new(ChainConfig {
            id: 56,
            name: "bsc-mainnet".to_string(),
            is_enabled: true,
            covalent_name: "bsc-mainnet".to_string(),
            rpc_url: "https://bsc-dataseed.binance.org".to_string(),
        });
        let chain_config2 = Arc::new(ChainConfig {
            id: 2,
            name: "eth-mainnet".to_string(),
            is_enabled: true,
            covalent_name: "ethereum".to_string(),
            rpc_url: "https://mainnet.infura.io/v3/".to_string(),
        });
        let mut chain_configs = HashMap::new();
        chain_configs.insert(56, chain_config1);
        chain_configs.insert(2, chain_config2);

        let token_config = Arc::new(TokenConfig {
            symbol: "USDT".to_string(),
            coingecko_symbol: "USDT".to_string(),
            is_enabled: true,
            by_chain: TokenConfigByChainConfigs(HashMap::new()),
        });
        let mut token_configs = HashMap::new();
        token_configs.insert("USDT".to_string(), token_config);

        let api_key = env::var("COINGECKO_API_KEY").expect("COINGECKO_API_KEY is not set");

        let store = KVStore::default();

        let client = CoingeckoClient::new(
            "https://api.coingecko.com/api/v3".to_string(),
            api_key,
            store,
            Duration::from_secs(300),
        );

        let config = get_sample_config();

        let routing_engine = RoutingEngine {
            aas_client,
            buckets,
            cache: Arc::new(RwLock::new(cache)),
            redis_client,
            estimates,
            chain_configs,
            token_configs,
            config: Arc::new(config),
            price_provider: Arc::new(Mutex::new(client)),
        };

        // should have USDT in bsc-mainnet > $0.5
        let dummy_user_address = "0x00000ebe3fa7cb71aE471547C836E0cE0AE758c2";
        let result = routing_engine
            .get_best_cost_paths(dummy_user_address, 2, "USDT", 0.5)
            .await
            .expect("Failed to get best cost path");
        assert_eq!(result.len(), 1);
        assert!(result[0].source_amount_in_usd > 0.49);
        assert!(result[0].from_address == dummy_user_address);
        Ok(())
    }
}
