use log::debug;
use std::sync::Arc;
use thiserror::Error;

use derive_more::Display;
use mongodb::bson;
use reqwest::Client as ReqwestClient;
use uuid::Uuid;

use storage::mongodb_client::{DBError, MongoDBClient};
use storage::DBProvider;

use crate::types::{
    Account, AddAccountPayload, ApiResponse, Balance, RegisterAccountPayload, User,
    UserAccountMapping, UserAccountMappingQuery, UserQuery,
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
            self.account_mapping_db_provider.read(&query).await?.ok_or_else(|| {
                AccountAggregationError::CustomError("User mapping not found".to_string())
            })?;
        Ok(Some(
            user_mapping
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
            .map_err(|e| AccountAggregationError::CustomError(e.to_string()))?;

        let accounts: Vec<Account> = accounts
            .iter()
            .filter_map(|account| {
                let account = account.as_document()?;
                let chain_id = account.get_str("chain_id").ok()?.to_string();
                let is_enabled = account.get_bool("is_enabled").ok()?;
                let account_address = account.get_str("account_address").ok()?.to_string();
                let account_type = account.get_str("account_type").ok()?.to_string();

                Some(Account { chain_id, is_enabled, account_address, account_type })
            })
            .collect();

        Ok(Some(accounts))
    }

    /// Register a new user account
    pub async fn register_user_account(
        &self,
        account_payload: RegisterAccountPayload,
    ) -> Result<(), AccountAggregationError> {
        let account = account_payload.account.to_lowercase();

        if self.get_user_id(&account).await?.is_none() {
            let user_id = Uuid::new_v4().to_string();
            let user_doc = self.user_db_provider.to_document(&User {
                user_id: user_id.clone(),
                accounts: vec![Account {
                    chain_id: account_payload.chain_id,
                    is_enabled: account_payload.is_enabled,
                    account_address: account.clone(),
                    account_type: account_payload.account_type,
                }],
            })?;

            self.user_db_provider.create(&user_doc).await?;

            let mapping_doc =
                self.account_mapping_db_provider.to_document(&UserAccountMapping {
                    account: account.clone(),
                    user_id: user_id.clone(),
                })?;
            self.account_mapping_db_provider.create(&mapping_doc).await?;
        } else {
            return Err(AccountAggregationError::CustomError(
                "Account already mapped to a user".to_string(),
            ));
        }
        Ok(())
    }

    /// Add an account to a user_id
    pub async fn add_account(
        &self,
        account_payload: AddAccountPayload,
    ) -> Result<(), AccountAggregationError> {
        let new_account = Account {
            chain_id: account_payload.chain_id.clone(),
            is_enabled: account_payload.is_enabled,
            account_address: account_payload.account.to_lowercase(),
            account_type: account_payload.account_type.clone(),
        };

        // Check if the account is already mapped to a user
        if self.get_user_id(&new_account.account_address).await?.is_some() {
            return Err(AccountAggregationError::CustomError(
                "Account already mapped to a user".to_string(),
            ));
        }

        // Fetch the user document
        let query_doc = self
            .user_db_provider
            .to_document(&UserQuery { user_id: account_payload.user_id.clone() })?;
        // Retrieve user document
        let mut user_doc =
            self.user_db_provider.read(&query_doc).await?.ok_or_else(|| {
                AccountAggregationError::CustomError("User not found".to_string())
            })?;

        // Add the new account to the user's accounts array
        let accounts_array =
            user_doc.entry("accounts".to_owned()).or_insert_with(|| bson::Bson::Array(vec![]));

        if let bson::Bson::Array(accounts) = accounts_array {
            accounts.push(bson::to_bson(&new_account)?);
        } else {
            return Err(AccountAggregationError::CustomError(
                "Failed to update accounts array".to_string(),
            ));
        }

        // Update the user document with the new account
        self.user_db_provider.update(&query_doc, &user_doc).await?;

        // Create a new mapping document for the account
        let mapping_doc = self.account_mapping_db_provider.to_document(&UserAccountMapping {
            account: new_account.account_address.clone(),
            user_id: account_payload.user_id.clone(),
        })?;
        self.account_mapping_db_provider.create(&mapping_doc).await?;

        Ok(())
    }

    /// Get the balance of a user's accounts
    pub async fn get_user_accounts_balance(
        &self,
        account: &String,
    ) -> Result<Vec<Balance>, AccountAggregationError> {
        let mut accounts: Vec<String> = Vec::new();
        let user_id = self.get_user_id(account).await.unwrap_or(None);
        if let Some(user_id) = user_id {
            let user_accounts = self.get_user_accounts(&user_id).await?.unwrap();
            accounts.extend(user_accounts.iter().map(|account| account.account_address.clone()));
        } else {
            accounts.push(account.clone());
        }

        let mut balances = Vec::new();
        let networks = self.networks.clone();
        debug!("Networks: {:?}", networks);

        // todo: parallelize this
        for user in accounts.iter() {
            for network in networks.iter() {
                let url = format!(
                    "{}/v1/{}/address/{}/balances_v2/?key={}",
                    self.covalent_base_url, network, user, self.covalent_api_key
                );
                debug!("Requesting: {}", url);
                let response = self.client.get(&url).send().await?;
                let api_response: ApiResponse = response.json().await?;
                let user_balances = extract_balance_data(api_response)?;
                balances.extend(user_balances);
            }
        }
        println!("{:?}", balances);

        Ok(balances)
    }
}

/// Extract balance data from the API response
fn extract_balance_data(
    api_response: ApiResponse,
) -> Result<Vec<Balance>, AccountAggregationError> {
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
