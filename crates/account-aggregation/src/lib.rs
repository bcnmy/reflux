use reqwest::{Client as ReqwestClient, Response};
use std::error::Error;
use std::sync::Arc;
use storage::StorageProvider;

mod types;

use crate::types::{Balance, ApiResponse};

#[derive(Clone)]
pub struct AccountAggregationService {
    pub storage: Arc<StorageProvider>,
    covalent_api_key: String,
}

impl AccountAggregationService {
    pub fn new(storage: StorageProvider, covalent_api_key: String) -> Self {
        Self {
            storage: Arc::new(storage),
            covalent_api_key,
        }
    }

    pub async fn get_user_accounts(&self, address: &String) -> Vec<String> {
        println!("Getting user accounts for address: {}", address);
        // storage get user accounts
        let users = self.storage.get_user_accounts(address).await;
        if let Some(users) = users {
            users
        } else {
            log::info!("No users found for address: {}", address);
            vec![address.clone()]
        }
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

        // collect all futures for all users & networks
        let fetches: Vec<_> = networks.iter().flat_map(|network| {
            let client = &client;
            users.iter().map(move |user| {
                let url = format!(
                    "https://api.covalenthq.com/v1/{}/address/{}/balances_v2/?key={}&no-spam=true&nft=false",
                    network, user, self.covalent_api_key
                );
                async move {
                    let res: Response = client
                        .get(&url)
                        .send()
                        .await
                        .map_err(Box::<dyn Error>::from)?;
                    let api_response: ApiResponse = res.json().await.map_err(Box::<dyn Error>::from)?;
                    extract_balance_data(api_response)
                }
            })
        }).collect();

        // wait for all futures to complete
        let results = futures::future::join_all(fetches).await;

        for result in results {
            match result {
                Ok(data) => balances.extend(data),
                Err(e) => eprintln!("Error fetching data: {}", e),
            }
        }
        balances
    }

    pub fn register_user_accounts(&mut self, user_id: String, accounts: Vec<String>) {
        // storage register user accounts
        // self.storage.register_user_accounts(user_id, accounts);
    }
}

fn extract_balance_data(api_response: ApiResponse) -> Result<Vec<Balance>, Box<dyn Error>> {
    let chain_id = api_response.data.chain_id.to_string();
    let results = api_response
        .data
        .items
        .iter()
        .filter_map(|item| {
            let token = &item
                .contract_ticker_symbol
                .clone()
                .unwrap_or("NONE".to_string());
            let balance_raw = item
                .balance
                .clone()
                .unwrap_or("0".to_string())
                .parse::<f64>()
                .map_err(Box::<dyn Error>::from)
                .ok()?;
            let quote = item.quote;

            if item.quote == None || item.quote == Some(0.0) || item.contract_decimals == None {
                // println!("Zero quote for token: {:?}", item);
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
