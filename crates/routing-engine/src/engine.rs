use account_aggregation::service::AccountAggregationService;
use account_aggregation::types::Balance;
use config::config::BucketConfig;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::estimator::{Estimator, LinearRegressionEstimator};

#[derive(Serialize, Deserialize, Debug, Display, PartialEq, Clone)]
#[display(
    "Route: from_chain: {}, to_chain: {}, token: {}, amount: {}",
    from_chain,
    to_chain,
    token,
    amount
)]
pub struct Route {
    pub from_chain: u32,
    pub to_chain: u32,
    pub token: String,
    pub amount: f64,
}

/// (from_chain, to_chain, from_token, to_token)
#[derive(Debug)]
struct PathQuery(u32, u32, String, String);

pub struct RoutingEngine {
    buckets: Vec<BucketConfig>,
    aas_client: AccountAggregationService,
    cache: Arc<HashMap<String, String>>, // (hash(bucket), hash(estimator_value)
}

impl RoutingEngine {
    pub fn new(aas_client: AccountAggregationService, buckets: Vec<BucketConfig>) -> Self {
        let cache = Arc::new(HashMap::new());

        Self { aas_client, cache, buckets }
    }

    /// Get the best cost path for a user
    /// This function will get the user balances from the aas and then calculate the best cost path for the user
    pub async fn get_best_cost_path(
        &self,
        account: &str,
        to_chain: u32,
        to_token: &str,
        to_value: f64,
    ) -> Vec<Route> {
        println!(
            "user: {}, to_chain: {}, to_token: {}, to_value: {}\n",
            account, to_chain, to_token, to_value
        );
        let user_balances = self.get_user_balance_from_agg_service(&account).await;
        // println!("User balances: {:?}\n", user_balances);

        // todo: for account aggregation, transfer same chain same asset first
        let direct_assets: Vec<_> =
            user_balances.iter().filter(|balance| balance.token == to_token).collect();
        // println!("\nDirect assets: {:?}\n", direct_assets);

        // Sort direct assets by A^x / C^y, here x=2 and y=1
        let x = 2.0;
        let y = 1.0;
        let mut sorted_assets: Vec<(&&Balance, f64)> = direct_assets
            .iter()
            .map(|balance| {
                let fee_cost = self.get_cached_data(
                    balance.amount_in_usd, // todo: edit
                    PathQuery(
                        balance.chain_id,
                        to_chain,
                        balance.token.to_string(),
                        to_token.to_string(),
                    ),
                );
                (balance, fee_cost)
            })
            .collect();

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
                token: balance.token.clone(),
                amount: amount_to_take,
            });
        }

        // Handle swap/bridge for remaining amount if needed (non direct assets)
        if total_amount_needed > 0.0 {
            let swap_assets: Vec<&Balance> =
                user_balances.iter().filter(|balance| balance.token != to_token).collect();
            let mut sorted_assets: Vec<(&&Balance, f64)> = swap_assets
                .iter()
                .map(|balance| {
                    let fee_cost = self.get_cached_data(
                        balance.amount_in_usd,
                        PathQuery(
                            balance.chain_id,
                            to_chain,
                            balance.token.clone(),
                            to_token.to_string(),
                        ),
                    );
                    (balance, fee_cost)
                })
                .collect();

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
                    token: balance.token.clone(),
                    amount: amount_to_take,
                });
            }
        }

        println!("\n Selected assets path: {:?}", selected_assets);
        println!("Total cost: {}", total_cost);

        selected_assets
    }

    fn get_cached_data(&self, target_amount: f64, path: PathQuery) -> f64 {
        // filter the bucket of (chain, token) and sort with token_amount_from_usd
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
            .unwrap();

        // todo: should throw error if not found in bucket range or unwrap or with last bucket
        let mut s = DefaultHasher::new();
        bucket.hash(&mut s);
        let key = s.finish().to_string();
        let value = self.cache.get(&key).unwrap();
        let estimator: Result<LinearRegressionEstimator, serde_json::Error> =
            serde_json::from_str(value);

        let cost = estimator.unwrap().borrow().estimate(target_amount);

        cost
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(&self, account: &str) -> Vec<Balance> {
        // Note: aas should always return vec of balances
        self.aas_client.get_user_accounts_balance(&account.to_string()).await
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::PathQuery;
    use crate::estimator::Estimator;
    use crate::{
        engine::RoutingEngine,
        estimator::{DataPoint, LinearRegressionEstimator},
    };
    use account_aggregation::service::AccountAggregationService;
    use config::BucketConfig;
    use std::env;
    use std::sync::Arc;
    use std::{
        collections::HashMap,
        hash::{DefaultHasher, Hash, Hasher},
    };
    use storage::mongodb_provider::MongoDBProvider;

    #[tokio::test]
    async fn test_get_cached_data() {
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
        let serialized_estimator = serde_json::to_string(&dummy_estimator).unwrap();

        // Create a cache with a dummy bucket
        let mut hasher = DefaultHasher::new();
        buckets[0].hash(&mut hasher);
        let key = hasher.finish().to_string();
        let mut cache = HashMap::new();
        cache.insert(key, serialized_estimator);

        // Create RoutingEngine instance with dummy data
        let user_db_provider = MongoDBProvider::new(
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
        let routing_engine = RoutingEngine { aas_client, buckets, cache: Arc::new(cache) };

        // Define the target amount and path query
        let target_amount = 5.0;
        let path_query = PathQuery(1, 2, "USDC".to_string(), "ETH".to_string());

        // Call get_cached_data and assert the result
        let result = routing_engine.get_cached_data(target_amount, path_query);
        assert!(result > 0.0);
        assert_eq!(result, dummy_estimator.estimate(target_amount));
    }

    #[tokio::test]
    async fn test_get_best_cost_path() {
        let api_key = env::var("COVALENT_API_KEY");
        if api_key.is_err() {
            panic!("COVALENT_API_KEY is not set");
        }
        let api_key = api_key.unwrap();

        let user_db_provider = MongoDBProvider::new(
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
        let serialized_estimator = serde_json::to_string(&dummy_estimator).unwrap();
        // Create a cache with a dummy bucket
        let mut hasher = DefaultHasher::new();
        buckets[0].hash(&mut hasher);
        let key = hasher.finish().to_string();
        let mut hasher = DefaultHasher::new();
        buckets[1].hash(&mut hasher);
        let key2 = hasher.finish().to_string();
        let mut cache = HashMap::new();
        cache.insert(key, serialized_estimator.clone());
        cache.insert(key2, serialized_estimator);
        let routing_engine = RoutingEngine { aas_client, buckets, cache: Arc::new(cache) };

        // should have USDT in bsc-mainnet > $0.5
        let dummy_user_address = "0x00000ebe3fa7cb71aE471547C836E0cE0AE758c2";
        let result = routing_engine.get_best_cost_path(dummy_user_address, 2, "USDT", 0.5).await;
        assert_eq!(result.len(), 1);
    }
}
