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
struct PathQuery(u32, u32, String, String);

// Todo: Define the following structs
// pub struct RouteSource;
// pub struct Estimator;
// pub struct EngineStore;
// pub struct MessageConsumer;

pub struct RoutingEngine {
    // route_sources: Vec<RouteSource>,
    // route_bucket_config: Vec<RouteBucketConfig>,
    buckets: Vec<BucketConfig>,
    // estimators: HashMap<String, Estimator>, // Concatenation of EstimatorType and RouteBucketConfig
    // engine_store: EngineStore,
    // message_consumer: MessageConsumer,
    aas_client: AccountAggregationService,
    // db_provider: Arc<dyn DBProvider + Send + Sync>,
    // settlement_engine: SettlementEngineClient,
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
        println!("\nDirect assets: {:?}\n", direct_assets);

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
            println!(
                "total_amount_needed: {}, balance.amount: {}, fee: {}",
                total_amount_needed, balance.amount, fee
            );
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

        let bucket = buckets_array.iter().find(|window| {
            target_amount >= window.token_amount_from_usd
                && target_amount <= window.token_amount_to_usd
        });

        // todo: should throw error if not found in bucket range or unwrap or with last bucket
        let mut s = DefaultHasher::new();
        bucket.hash(&mut s);
        let key = s.finish().to_string();

        let value = self.cache.get(&key).unwrap();

        let estimator: Result<LinearRegressionEstimator, serde_json::Error> =
            serde_json::from_str(value);

        let cost = estimator.unwrap().borrow().estimate(target_amount);
        println!("Cost for the target_amount {}", cost);

        cost
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(&self, account: &str) -> Vec<Balance> {
        // Note: aas should always return vec of balances
        self.aas_client.get_user_accounts_balance(&account.to_string()).await
    }
}
