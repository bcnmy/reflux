use futures::future::join_all;
use log::debug;
use std::sync::Arc;

use derive_more::Display;
use mongodb::bson;
use reqwest::Client as ReqwestClient;
use thiserror::Error;
use uuid::Uuid;

use storage::DBProvider;
use storage::mongodb_client::{DBError, MongoDBClient};

use crate::types::{
    Account, AddAccountPayload, CovalentApiResponse, TokenWithBalance, RegisterAccountPayload,
    User, UserAccountMapping, UserAccountMappingQuery, UserQuery,
};

#[derive(Error, Debug)]
pub enum AccountAggregationError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DBError),

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] bson::ser::Error),

    #[error("Deserialization error: {0}")]
    DeserializationError(#[from] bson::de::Error),

    #[error("Custom error: {0}")]
    CustomError(String),
}

/// Account Aggregation Service
///
/// This service is responsible for managing user accounts and their balances
/// It interacts with the user and account mapping databases to store and retrieve user account information
#[derive(Clone, Display, Debug)]
#[display(
"AccountAggregationService {{ user_db_provider: {:?}, account_mapping_db_provider: {:?} }}",
    user_db_provider,
    account_mapping_db_provider
)]
pub struct AccountAggregationService {
    pub user_db_provider: Arc<MongoDBClient>,
    pub account_mapping_db_provider: Arc<MongoDBClient>,
    covalent_base_url: String,
    covalent_api_key: String,
    client: ReqwestClient,
    networks: Vec<String>,
}

impl AccountAggregationService {
    /// Create a new AccountAggregationService
    pub fn new(
        user_db_provider: MongoDBClient,
        account_mapping_db_provider: MongoDBClient,
        networks: Vec<String>,
        base_url: String,
        api_key: String,
    ) -> Self {
        // todo: should add the arc once here for multiple tasks
        let reqwest_client = ReqwestClient::new();
        Self {
            user_db_provider: Arc::new(user_db_provider),
            account_mapping_db_provider: Arc::new(account_mapping_db_provider),
            covalent_base_url: base_url,
            covalent_api_key: api_key,
            client: reqwest_client,
            networks,
        }
    }

    /// Get the user_id associated with an account
    pub async fn get_user_id(
        &self,
        account: &String,
    ) -> Result<Option<String>, AccountAggregationError> {
        let account = account.to_lowercase();
        let query =
            self.account_mapping_db_provider.to_document(&UserAccountMappingQuery { account })?;
        let user_mapping =
            self.account_mapping_db_provider.read(&query).await.unwrap_or(None);
        if user_mapping.is_none() {
            return Ok(None);
        }
        Ok(Some(
            user_mapping
                .unwrap()
                .get_str("user_id")
                .map_err(|e| AccountAggregationError::CustomError(e.to_string()))?
                .to_string(),
        ))
    }

    /// Get the accounts associated with a user_id
    pub async fn get_user_accounts(
        &self,
        user_id: &String,
    ) -> Result<Option<Vec<Account>>, AccountAggregationError> {
        let query = self.user_db_provider.to_document(&UserQuery { user_id: user_id.clone() })?;

        let user =
            self.user_db_provider.read(&query).await?.ok_or_else(|| {
                AccountAggregationError::CustomError("User not found".to_string())
            })?;
        let accounts = user
            .get_array("accounts")
            .map_err(|e| AccountAggregationError::CustomError(e.to_string()));

        let accounts: Vec<Account> = accounts?
            .iter()
            .filter_map(|account| {
                let account = account.as_document()?;
                // let chain_id = account.get_str("chain_id").ok()?.to_string();
                let address = account.get_str("address").ok()?.to_string();
                let is_enabled = account.get_bool("is_enabled").ok()?;
                let account_type = account.get_str("account_type").ok()?.to_string();
                let tags = account
                    .get_array("tags")
                    .map(|tags| {
                        tags.iter()
                            .filter_map(|tag| tag.as_str().map(|tag| tag.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                Some(Account { address, is_enabled, account_type, tags })
            })
            .collect();

        Ok(Some(accounts))
    }

    /// Register a new user account
    pub async fn register_user_account(
        &self,
        account_payload: RegisterAccountPayload,
    ) -> Result<(), AccountAggregationError> {
        // Modify all accounts address to lowercase
        let all_accounts: Vec<_> = account_payload
            .accounts
            .into_iter()
            .map(|account| Account {
                address: account.address.to_lowercase(),
                account_type: account.account_type,
                is_enabled: account.is_enabled,
                tags: account.tags,
            })
            .collect();

        for account in all_accounts.iter() {
            let user_id = self.get_user_id(&account.address).await;
            match user_id {
                Ok(Some(_)) => {
                    return Err(AccountAggregationError::CustomError(
                        "Account already mapped to a user".to_string(),
                    ));
                }
                Ok(None) => {}
                Err(_e) => {}
            }
        }

        let user_id = Uuid::new_v4().to_string();
        let user = User { user_id: user_id.clone(), accounts: all_accounts.clone() };
        let user_doc = self.user_db_provider.to_document(&user)?;
        self.user_db_provider.create(&user_doc).await?;

        for account in all_accounts {
            let mapping_doc =
                self.account_mapping_db_provider.to_document(&UserAccountMapping {
                    user_id: user_id.clone(),
                    account: account.address.clone(),
                })?;
            self.account_mapping_db_provider.create(&mapping_doc).await?;
        }

        Ok(())
    }

    /// Add an account to a user_id
    pub async fn add_account(
        &self,
        account_payload: AddAccountPayload,
    ) -> Result<(), AccountAggregationError> {
        // Fetch the user document
        let query_doc = self
            .user_db_provider
            .to_document(&UserQuery { user_id: account_payload.user_id.clone().unwrap() })?;
        let mut user_doc =
            self.user_db_provider.read(&query_doc).await?.ok_or_else(|| {
                AccountAggregationError::CustomError("User not found".to_string())
            })?;

        // Convert all account addresses to lowercase
        let mut new_accounts = vec![];
        for account in account_payload.account {
            new_accounts.push(Account {
                address: account.address.to_lowercase(),
                account_type: account.account_type,
                is_enabled: account.is_enabled,
                tags: account.tags,
            });
        }

        // Add the new accounts to the user's accounts array
        let accounts_array =
            user_doc.entry("accounts".to_owned()).or_insert_with(|| bson::Bson::Array(vec![]));

        if let bson::Bson::Array(accounts) = accounts_array {
            for new_account in new_accounts.iter() {
                if self.get_user_id(&new_account.address).await?.is_some() {
                    return Err(AccountAggregationError::CustomError(
                        "Account already mapped to a user".to_string(),
                    ));
                }
                accounts.push(bson::to_bson(new_account)?);
            }
        } else {
            return Err(AccountAggregationError::CustomError(
                "Failed to update accounts array".to_string(),
            ));
        }

        // Update the user document with the new accounts
        self.user_db_provider.update(&query_doc, &user_doc).await?;

        // Create a new mapping document for each account
        for new_account in new_accounts {
            let mapping_doc =
                self.account_mapping_db_provider.to_document(&UserAccountMapping {
                    account: new_account.address.clone(),
                    user_id: account_payload.user_id.clone().unwrap(),
                })?;
            self.account_mapping_db_provider.create(&mapping_doc).await?;
        }

        Ok(())
    }

    /// Get the balance of a user's accounts
    pub async fn get_user_accounts_balance(
        &self,
        account: &String,
    ) -> Result<Vec<TokenWithBalance>, AccountAggregationError> {
        let mut accounts: Vec<String> = Vec::new();
        let user_id = self.get_user_id(account).await.unwrap_or(None);
        if let Some(user_id) = user_id {
            let user_accounts = self.get_user_accounts(&user_id).await?.unwrap();
            accounts.extend(user_accounts.iter().map(|account| account.address.clone()));
        } else {
            accounts.push(account.clone());
        }

        let mut balances = Vec::new();
        let networks = self.networks.clone();

        // Prepare tasks for parallel execution
        let tasks: Vec<_> = accounts
            .iter()
            .flat_map(|user| {
                networks.iter().map(move |network| {
                    let url = format!(
                        "{}/v1/{}/address/{}/balances_v2/?key={}",
                        self.covalent_base_url, network, user, self.covalent_api_key
                    );
                    let client = self.client.clone();
                    async move {
                        let response = client.get(&url).send().await;
                        match response {
                            Ok(response) => {
                                let api_response: Result<CovalentApiResponse, _> =
                                    response.json().await;
                                match api_response {
                                    Ok(api_response) => extract_balance_data(api_response),
                                    Err(e) => Err(AccountAggregationError::ReqwestError(e)),
                                }
                            }
                            Err(e) => Err(AccountAggregationError::ReqwestError(e)),
                        }
                    }
                })
            })
            .collect();

        // Execute tasks concurrently
        let results = join_all(tasks).await;

        // Collect results
        for result in results {
            match result {
                Ok(user_balances) => balances.extend(user_balances),
                Err(e) => debug!("Failed to fetch balance: {:?}", e),
            }
        }

        Ok(balances)
    }
}

/// Extract balance data from the API response
fn extract_balance_data(
    api_response: CovalentApiResponse,
) -> Result<Vec<TokenWithBalance>, AccountAggregationError> {
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
                .map_err(|e| AccountAggregationError::CustomError(e.to_string()))
                .ok()?;
            let quote = item.quote;

            if item.quote == None || item.quote == Some(0.0) || item.contract_decimals == None {
                None
            } else {
                let balance = balance_raw / 10f64.powf(item.contract_decimals.unwrap() as f64);

                Some(TokenWithBalance {
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
