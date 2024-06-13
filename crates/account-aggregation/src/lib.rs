use crate::types::{ApiResponse, Balance};
use mongodb::bson::{doc, Document};
use reqwest::{Client as ReqwestClient, Response};
use std::error::Error;
use std::sync::Arc;
use storage::db_provider::DBProvider;
use uuid::Uuid;

mod types;

#[derive(Clone)]
pub struct AccountAggregationService<T: DBProvider + Send + Sync + Clone + 'static> {
    pub user_db_provider: Arc<T>,
    pub account_mapping_db_provider: Arc<T>,
    covalent_base_url: String,
    covalent_api_key: String,
}

impl<T: DBProvider + Send + Sync + Clone + 'static> AccountAggregationService<T> {
    pub fn new(
        user_db_provider: T,
        account_mapping_db_provider: T,
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

    pub async fn get_user_id(&self, address: &String) -> Option<String> {
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

    pub async fn get_user_accounts(&self, address: &String) -> Result<Vec<String>, Box<dyn Error>> {
        if let Some(user_id) = self.get_user_id(address).await {
            let query = doc! { "user_id": &user_id };
            if let Ok(Some(user)) = self.user_db_provider.read::<Document>(&query).await {
                if let Some(accounts) = user.get_array("accounts").ok() {
                    return Ok(accounts
                        .iter()
                        .filter_map(|acc| acc.as_str().map(String::from))
                        .collect());
                }
            }
        }
        Ok(vec![address.clone()])
    }

    pub async fn register_user_account(
        &self,
        address: String,
        account_type: String,
        chain_id: String,
        is_enabled: bool,
    ) -> Result<(), Box<dyn Error>> {
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

    pub async fn get_user_accounts_balance(&self, users: Vec<String>) -> Vec<Balance> {
        let mut balances = Vec::new();
        let client = Arc::new(ReqwestClient::new());
        let networks = [
            "eth-mainnet",
            "matic-mainnet",
            "base-mainnet",
            "blast-mainnet",
            "arbitrum-mainnet",
            "optimism-mainnet",
        ];

        let fetches: Vec<_> = networks
            .iter()
            .flat_map(|network| {
                let client = &client;
                users.iter().map(move |user| {
                    let url = format!(
                        "{}/{}/address/{}/balances_v2/?key={}&no-spam=true&nft=false",
                        self.covalent_base_url, network, user, self.covalent_api_key
                    );
                    async move {
                        let res: Response =
                            client.get(&url).send().await.map_err(Box::<dyn Error>::from)?;
                        let api_response: ApiResponse =
                            res.json().await.map_err(Box::<dyn Error>::from)?;
                        extract_balance_data(api_response)
                    }
                })
            })
            .collect();

        let results = futures::future::join_all(fetches).await;

        for result in results {
            match result {
                Ok(data) => balances.extend(data),
                Err(e) => eprintln!("Error fetching data: {}", e),
            }
        }
        balances
    }
}

fn extract_balance_data(api_response: ApiResponse) -> Result<Vec<Balance>, Box<dyn Error>> {
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

    Ok(results)
}
