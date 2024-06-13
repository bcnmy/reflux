use crate::route_fee_bucket::RouteFeeBucket;
use account_aggregation::types::Balance;
use account_aggregation::AccountAggregationService;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug, Display, PartialEq, Clone)]
#[display(
    "Route: from_chain: {}, to_chain: {}, token: {}, amount: {}, path: {:?}",
    from_chain,
    to_chain,
    token,
    amount,
    path
)]
pub struct Route {
    pub from_chain: u32,
    pub to_chain: u32,
    pub token: String,
    pub amount: f64,
    pub path: Vec<String>,
}

// Todo: Define the following structs
pub struct RouteSource;
pub struct RouteBucketConfig;
pub struct Estimator;
pub struct EngineStore;
pub struct MessageConsumer;

pub struct RoutingEngine {
    // route_sources: Vec<RouteSource>,
    // route_bucket_config: Vec<RouteBucketConfig>,
    // buckets: Vec<RouteFeeBucket>,
    // estimators: HashMap<String, Estimator>, // Concatenation of EstimatorType and RouteBucketConfig
    // engine_store: EngineStore,
    // message_consumer: MessageConsumer,
    aas_client: AccountAggregationService,
    // db_provider: Arc<dyn DBProvider + Send + Sync>,
    // settlement_engine: SettlementEngineClient,
    _cache: Arc<HashMap<String, (f64, f64, String)>>, // (key, (fee, quote, bridge_name))
    route_fee_bucket: RouteFeeBucket,
}

impl RoutingEngine {
    pub fn new(aas_client: AccountAggregationService) -> Self {
        let cache = Arc::new(HashMap::new());
        let route_fee_bucket = RouteFeeBucket::new();
        Self { aas_client, _cache: cache, route_fee_bucket }
    }

    /// Get the best cost path for a user
    /// This function will get the user balances from the aas and then calculate the best cost path for the user
    pub async fn get_best_cost_path(
        &self,
        user_id: &str,
        to_chain: u32,
        to_token: &str,
        to_value: f64,
    ) -> Vec<Route> {
        println!(
            "user: {}, to_chain: {}, to_token: {}, to_value: {}\n",
            user_id, to_chain, to_token, to_value
        );
        let user_balances = self.get_user_balance_from_agg_service(user_id).await;
        println!("User balances: {:?}\n", user_balances);

        // todo: for account aggregation, transfer same chain same asset first
        let direct_assets: Vec<_> =
            user_balances.iter().filter(|balance| balance.token == to_token).collect();
        println!("\n ---Direct assets: {:?}\n", direct_assets);

        // Sort direct assets by A^x / C^y, here x=2 and y=1
        let x = 2.0;
        let y = 1.0;
        let mut sorted_assets: Vec<_> = direct_assets
            .iter()
            .map(|balance| {
                let (fee, quote, bridge_name) = self.route_fee_bucket.get_cost(
                    balance.chain_id,
                    to_chain,
                    &balance.token,
                    to_token,
                );
                (balance, fee, quote, bridge_name)
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

        for (balance, fee, _quote, bridge_name) in sorted_assets {
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
                path: vec![bridge_name.clone()],
            });
        }

        // Handle swap/bridge for remaining amount if needed
        if total_amount_needed > 0.0 {
            let mut swap_assets: Vec<_> =
                user_balances.iter().filter(|balance| balance.token != to_token).collect();
            swap_assets.sort_by(|a, b| {
                // let key_a = format!("{}_{}_{}_{}", a.chain_id, to_chain, &a.token, to_token);
                // let key_b = format!("{}_{}_{}_{}", b.chain_id, to_chain, &b.token, to_token);
                let (fee_a, quote_a, _) =
                    self.route_fee_bucket.get_cost(a.chain_id, to_chain, &a.token, to_token);
                let (fee_b, quote_b, _) =
                    self.route_fee_bucket.get_cost(b.chain_id, to_chain, &b.token, to_token);
                let cost_a = (quote_a.powf(x)) / (fee_a.powf(y));
                let cost_b = (quote_b.powf(x)) / (fee_b.powf(y));
                cost_a.partial_cmp(&cost_b).unwrap()
            });

            for balance in swap_assets {
                if total_amount_needed <= 0.0 {
                    break;
                }

                let (fee, quote, bridge_name) = self.route_fee_bucket.get_cost(
                    balance.chain_id,
                    to_chain,
                    &balance.token,
                    to_token,
                );
                let amount_to_take =
                    if quote >= total_amount_needed { total_amount_needed } else { quote };

                total_amount_needed -= amount_to_take;
                total_cost += fee;

                selected_assets.push(Route {
                    from_chain: balance.chain_id,
                    to_chain,
                    token: balance.token.clone(),
                    amount: amount_to_take,
                    path: vec![bridge_name.clone()],
                });
            }
        }

        println!("\n ---Selected assets path: {:?}", selected_assets);
        println!("Total cost: {}", total_cost);

        selected_assets
    }

    /// Get user balance from account aggregation service
    async fn get_user_balance_from_agg_service(&self, user_id: &str) -> Vec<Balance> {
        // Note: aas should always return vec of balances
        self.aas_client.get_user_accounts_balance(&user_id.to_string()).await
    }
}
