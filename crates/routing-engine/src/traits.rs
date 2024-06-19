use account_aggregation::types::Balance;
use async_trait::async_trait;
use std::error::Error;

#[async_trait]
pub trait AccountAggregation {
    async fn get_user_accounts_balance(
        &self,
        user_id: &String,
    ) -> Result<Vec<Balance>, Box<dyn Error>>;
}

pub trait RouteFee {
    fn get_cost(
        &self,
        from_chain: u32,
        to_chain: u32,
        from_token: &str,
        to_token: &str,
    ) -> (f64, f64, String);
}
