use async_trait::async_trait;
use storage::mongodb_provider::MongoDBProvider;
use crate::types::{Account, AddAccountPayload, Balance, RegisterAccountPayload};
use std::error::Error;

#[async_trait]
pub trait AccountAggregationServiceTrait {
    fn new(
        user_db_provider: MongoDBProvider,
        account_mapping_db_provider: MongoDBProvider,
        base_url: String,
        api_key: String,
    ) -> Self;
    async fn get_user_id(&self, account: &String) -> Option<String>;
    fn get_user_accounts(&self, user_id: &String) -> Option<Vec<Account>>;
    fn register_user_account(
        &self,
        account_payload: RegisterAccountPayload,
    ) -> Result<(), Box<dyn Error>>;
    fn add_account(&self, account_payload: AddAccountPayload) -> Result<(), Box<dyn Error>>;
    fn get_user_accounts_balance(&self, account: &String) -> Vec<Balance>;
}
