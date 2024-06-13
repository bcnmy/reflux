use mongodb::bson::{doc, Document};
use reqwest::Client as ReqwestClient;
use std::error::Error;
use std::sync::Arc;
use storage::db_provider::DBProvider;
use storage::mongodb_provider::MongoDBProvider;
use types::{ApiResponse, Balance};
use uuid::Uuid;

pub mod types;

#[derive(Clone)]
pub struct AccountAggregationService {
    pub user_db_provider: Arc<MongoDBProvider>,
    pub account_mapping_db_provider: Arc<MongoDBProvider>,
    covalent_base_url: String,
    covalent_api_key: String,
}

impl AccountAggregationService {
    /// Create a new AccountAggregationService
    pub fn new(
        user_db_provider: MongoDBProvider,
        account_mapping_db_provider: MongoDBProvider,
        base_url: String,
        api_key: String,
    ) -> Self {
        Self {
            user_db_provider: Arc::new(user_db_provider),
            account_mapping_db_provider: Arc::new(account_mapping_db_provider),
            covalent_base_url: base_url,
            covalent_api_key: api_key,
        }
    }

    /// Get the user_id associated with an address
    pub async fn get_user_id(&self, address: &String) -> Option<String> {
        let address = address.to_lowercase();
        let query = doc! { "account": address };
        if let Ok(Some(user_mapping)) =
            self.account_mapping_db_provider.read::<Document>(&query).await
        {
            if let Some(user_id) = user_mapping.get_str("user_id").ok() {
                return Some(user_id.to_string());
            }
        }
        None
    }

    /// Get the accounts associated with a user_id
    pub async fn get_user_accounts(&self, user_id: &String) -> Option<Vec<String>> {
        let query = doc! { "user_id": user_id };
        if let Ok(Some(user)) = self.user_db_provider.read::<Document>(&query).await {
            if let Some(accounts) = user.get_array("accounts").ok() {
                let accounts: Vec<String> = accounts
                    .iter()
                    .filter_map(|account| account.as_str().map(|s| s.to_string()))
                    .collect();
                return Some(accounts);
            }
        }
        None
    }

    /// Register a new user account
    /// todo: update the schema here, need chainId?
    pub async fn register_user_account(
        &self,
        address: String,
        account_type: String,
        chain_id: String,
        is_enabled: bool,
    ) -> Result<(), Box<dyn Error>> {
        let address = address.to_lowercase();

        if self.get_user_id(&address).await.is_none() {
            let user_id = Uuid::new_v4().to_string();
            let user_doc = doc! {
                "user_id": &user_id,
                "type": account_type.clone(),
                "accounts": [address.clone()],
                "chain_id": chain_id,
                "is_enabled": is_enabled
            };
            self.user_db_provider.create(&user_doc).await?;

            let mapping_doc = doc! {
                "account": &address,
                "user_id": user_id
            };
            self.account_mapping_db_provider.create(&mapping_doc).await?;
        }
        Ok(())
    }

    /// Add an account to a user_id
    /// todo: same as above
    pub async fn add_account(
        &self,
        user_id: String,
        new_account: String,
        account_type: String,
        chain_id: String,
        is_enabled: bool,
    ) -> Result<(), Box<dyn Error>> {
        let query = doc! { "user_id": &user_id };
        let update = doc! {
            "$addToSet": { "accounts": new_account.clone() },
            "$set": { "type": account_type, "chain_id": chain_id, "is_enabled": is_enabled }
        };
        self.user_db_provider.update(&query, &update).await?;

        let mapping_doc = doc! {
            "account": new_account,
            "user_id": user_id
        };
        self.account_mapping_db_provider.create(&mapping_doc).await?;
        Ok(())
    }

    /// Get the balance of a user's accounts
    pub async fn get_user_accounts_balance(&self, user_id: &String) -> Vec<Balance> {
        // find the users accounts from the user_id
        let query = doc! { "user_id": user_id };
        let user = self.user_db_provider.read::<Document>(&query).await.unwrap().unwrap();
        let accounts = user.get_array("accounts").unwrap();
        let users: Vec<String> =
            accounts.iter().filter_map(|account| account.as_str().map(|s| s.to_string())).collect();

        let mut balances = Vec::new();
        let client = Arc::new(ReqwestClient::new());
        let networks = [
            // "eth-mainnet",
            "matic-mainnet",
            // "base-mainnet",
            // "blast-mainnet",
            "arbitrum-mainnet",
            "optimism-mainnet",
        ];
        println!("{:?}", users);

        // todo: parallelize this
        for network in networks.iter() {
            for user in users.iter() {
                let url = format!(
                    "{}/v1/{}/address/{}/balances_v2/?key={}",
                    self.covalent_base_url, network, user, self.covalent_api_key
                );
                let response = client.get(&url).send().await.unwrap();
                let api_response: ApiResponse = response.json().await.unwrap();
                let user_balances = extract_balance_data(api_response).unwrap();
                balances.extend(user_balances);
            }
        }

        balances
    }
}

/// Extract balance data from the API response
fn extract_balance_data(api_response: ApiResponse) -> Option<Vec<Balance>> {
    let chain_id = api_response.data.chain_id.to_string();
    let results = api_response
        .data
        .items
        .iter()
        .filter_map(|item| {
            let token = &item.contract_ticker_symbol.clone().unwrap_or("NONE".to_string());
            let balance_raw = item
                .balance
                .clone()
                .unwrap_or("0".to_string())
                .parse::<f64>()
                .map_err(Box::<dyn Error>::from)
                .ok()?;
            let quote = item.quote;

            if item.quote == None || item.quote == Some(0.0) || item.contract_decimals == None {
                None
            } else {
                let balance = balance_raw / 10f64.powf(item.contract_decimals.unwrap() as f64);

                Some(Balance {
                    token: token.clone(),
                    token_address: item.contract_ticker_symbol.clone().unwrap(),
                    chain_id: chain_id.clone().parse::<u32>().unwrap(),
                    amount: balance,
                    amount_in_usd: quote.unwrap(),
                })
            }
        })
        .collect();

    Some(results)
}
