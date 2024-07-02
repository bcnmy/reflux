use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Deref;

use derive_more::{Display, From, Into};
use serde::Deserialize;
use serde_valid::yaml::FromYamlStr;
use serde_valid::{UniqueItemsError, Validate, ValidateUniqueItems};

// Config Type
#[derive(Debug)]
pub struct Config {
    // A bucket is defined for a pair of (source chain, source token) and (destination chain, destination token)
    // in which the estimation algorithm will be applied.
    pub buckets: Vec<BucketConfig>,
    // List of all chains and their configurations.
    pub chains: HashMap<u32, ChainConfig>,
    // List of all tokens and their configurations.
    pub tokens: HashMap<String, TokenConfig>,
    // Bungee API configuration
    pub bungee: BungeeConfig,
    // CoinGecko API configuration
    pub coingecko: CoinGeckoConfig,
    // Covalent API configuration
    pub covalent: CovalentConfig,
    // Infra Dependencies
    pub infra: InfraConfig,
    // API Server Configuration
    pub server: ServerConfig,
    // Configuration for the indexer
    pub indexer_config: IndexerConfig,
    // Configuration for the solver
    pub solver_config: SolverConfig,
}

impl Config {
    pub fn from_file(file_path: &str) -> Result<Self, ConfigError> {
        let config_file_content = std::fs::read_to_string(file_path)?;
        Self::from_yaml_str(&config_file_content)
    }

    pub fn from_yaml_str(s: &str) -> Result<Self, ConfigError> {
        let raw_config = RawConfig::from_yaml_str(s)?;
        let mut chains = HashMap::new();
        for chain in raw_config.chains.0 {
            chains.insert(chain.id, chain);
        }

        let mut tokens = HashMap::new();

        fn verify_chain(
            chain_id: u32,
            chains: &HashMap<u32, ChainConfig>,
        ) -> Result<(), ConfigError> {
            if let Some(chain) = chains.get(&chain_id) {
                if !chain.is_enabled {
                    return Err(ConfigError::ChainNotSupported(chain_id));
                }
            } else {
                return Err(ConfigError::ChainNotFound(chain_id));
            }
            Ok(())
        }

        fn verify_token(
            token_symbol: &str,
            chain_id: u32,
            tokens: &HashMap<String, TokenConfig>,
        ) -> Result<(), ConfigError> {
            if let Some(token) = tokens.get(token_symbol) {
                if !token.is_enabled {
                    return Err(ConfigError::TokenNotSupported(token_symbol.to_string()));
                }

                if let Some(chain_config) = token.by_chain.get(&chain_id) {
                    if !chain_config.is_enabled {
                        return Err(ConfigError::TokenNotSupportedOnChain(
                            token_symbol.to_string(),
                            chain_id,
                        ));
                    }
                } else {
                    return Err(ConfigError::TokenNotFoundOnChain(
                        token_symbol.to_string(),
                        chain_id,
                    ));
                }
            } else {
                return Err(ConfigError::TokenNotFound(token_symbol.to_string()));
            }
            Ok(())
        }

        // Validate chains in the token configuration
        for token in raw_config.tokens.0 {
            for (chain_id, _) in token.by_chain.iter() {
                if let Err(e) = verify_chain(*chain_id, &chains) {
                    return Err(e);
                }
            }

            tokens.insert(token.symbol.clone(), token);
        }

        // Validate chains and tokens in the bucket configuration
        for bucket in raw_config.buckets.0.iter() {
            if let Err(e) = verify_chain(bucket.from_chain_id, &chains) {
                return Err(e);
            }
            if let Err(e) = verify_chain(bucket.to_chain_id, &chains) {
                return Err(e);
            }
            if let Err(e) = verify_token(&bucket.from_token, bucket.from_chain_id, &tokens) {
                return Err(e);
            }
            if let Err(e) = verify_token(&bucket.to_token, bucket.to_chain_id, &tokens) {
                return Err(e);
            }
        }

        Ok(Config {
            chains,
            tokens,
            buckets: raw_config.buckets.0,
            covalent: raw_config.covalent,
            bungee: raw_config.bungee,
            coingecko: raw_config.coingecko,
            infra: raw_config.infra,
            server: raw_config.server,
            indexer_config: raw_config.indexer_config,
            solver_config: raw_config.solver_config,
        })
    }
}

#[derive(Debug, From, Display)]
pub enum ConfigError {
    #[display("Chain not supported: {}", _0)]
    #[from(ignore)]
    ChainNotSupported(u32),

    #[display("Chain not found: {}", _0)]
    #[from(ignore)]
    ChainNotFound(u32),

    #[display("Token not supported: {}", _0)]
    #[from(ignore)]
    TokenNotSupported(String),

    #[display("Token not supported: {} on chain: {}", _0, _1)]
    #[from(ignore)]
    TokenNotSupportedOnChain(String, u32),

    #[display("Token not found: {}", _0)]
    #[from(ignore)]
    TokenNotFound(String),

    #[display("Token not found: {} on chain: {}", _0, _1)]
    #[from(ignore)]
    TokenNotFoundOnChain(String, u32),

    #[display("Serde Error: {}", _0)]
    SerdeError(serde_valid::Error<serde_yaml::Error>),

    #[display("Error Reading Config File: {}", _0)]
    IoError(std::io::Error),
}

// Intermediate Config Type as Deserialization Target
#[derive(Debug, Deserialize, From, Into)]
pub struct Buckets(Vec<BucketConfig>);

impl ValidateUniqueItems for Buckets {
    fn validate_unique_items(&self) -> Result<(), UniqueItemsError> {
        self.0
            .iter()
            .map(|b| {
                let mut s = DefaultHasher::new();
                b.hash(&mut s);
                s.finish()
            })
            .collect::<Vec<_>>()
            .validate_unique_items()
    }
}

#[derive(Debug, Deserialize, From, Into)]
pub struct Chains(Vec<ChainConfig>);

impl ValidateUniqueItems for Chains {
    fn validate_unique_items(&self) -> Result<(), UniqueItemsError> {
        self.iter().map(|c| c.id).collect::<Vec<_>>().validate_unique_items()
    }
}

impl Deref for Chains {
    type Target = Vec<ChainConfig>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Deserialize, From, Into)]
pub struct TokenConfigs(Vec<TokenConfig>);
impl ValidateUniqueItems for TokenConfigs {
    fn validate_unique_items(&self) -> Result<(), UniqueItemsError> {
        self.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>().validate_unique_items()
    }
}

impl Deref for TokenConfigs {
    type Target = Vec<TokenConfig>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Deserialize, Validate)]
pub struct RawConfig {
    #[validate(unique_items)]
    pub buckets: Buckets,
    #[validate(unique_items)]
    pub chains: Chains,
    #[validate(unique_items)]
    pub tokens: TokenConfigs,
    pub bungee: BungeeConfig,
    pub coingecko: CoinGeckoConfig,
    pub covalent: CovalentConfig,
    pub infra: InfraConfig,
    pub server: ServerConfig,
    pub indexer_config: IndexerConfig,
    pub solver_config: SolverConfig,
}

#[derive(Debug, Deserialize, Validate, PartialOrd, Clone)]
pub struct BucketConfig {
    // The source chain
    #[validate(minimum = 1)]
    pub from_chain_id: u32,
    // The destination chain
    #[validate(minimum = 1)]
    pub to_chain_id: u32,
    // The source token
    #[validate(min_length = 1)]
    pub from_token: String,
    // The destination token
    #[validate(min_length = 1)]
    pub to_token: String,
    // Whether the bucket should only index routes that support smart contracts or just EOAs
    pub is_smart_contract_deposit_supported: bool,
    // Lower bound of the token amount to be transferred from the source chain to the destination chain
    #[validate(minimum = 1.0)]
    pub token_amount_from_usd: f64,
    // Upper bound of the token amount to be transferred from the source chain to the destination chain
    #[validate(minimum = 1.0)]
    pub token_amount_to_usd: f64,
}

impl Ord for BucketConfig {
    // sort with token_amount_from_usd
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.token_amount_from_usd.partial_cmp(&other.token_amount_from_usd).unwrap()
    }
}

impl BucketConfig {
    pub fn get_hash(&self) -> u64 {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish()
    }
}

// Implementation for treating a BucketConfig as a key in a k-v pair
impl Hash for BucketConfig {
    fn hash<H: Hasher>(&self, state: &mut H) {
        const PRECISION: u32 = 5;

        self.from_chain_id.hash(state);
        self.to_chain_id.hash(state);
        self.from_token.hash(state);
        self.to_token.hash(state);
        self.is_smart_contract_deposit_supported.hash(state);

        // We limit the precision of amounts to <PRECISION> decimal places
        ((self.token_amount_from_usd * 10_f64.powi(PRECISION as i32)) as i64).hash(state);
        ((self.token_amount_to_usd * 10_f64.powi(PRECISION as i32)) as i64).hash(state);
    }
}

impl PartialEq<Self> for BucketConfig {
    fn eq(&self, other: &Self) -> bool {
        self.get_hash() == other.get_hash()
    }
}

impl Eq for BucketConfig {}

#[derive(Debug, Deserialize, Validate, Clone)]
pub struct ChainConfig {
    // The chain id
    #[validate(minimum = 1)]
    pub id: u32,
    // The name of the chain
    #[validate(min_length = 1)]
    pub name: String,
    // If the chain is enabled or now
    pub is_enabled: bool,
    // The name of the chain in Covalent API
    #[validate(min_length = 1)]
    pub covalent_name: String,
}

#[derive(Debug, Deserialize, Validate, Clone)]
pub struct TokenConfig {
    // The token symbol
    #[validate(min_length = 1)]
    pub symbol: String,
    // The symbol of the token in coingecko API
    #[validate(min_length = 1)]
    pub coingecko_symbol: String,
    // Whether the token across chains is supported
    pub is_enabled: bool,
    // Chain Specific Configuration
    #[validate(unique_items)]
    pub by_chain: TokenConfigByChainConfigs,
}

#[derive(Debug, Deserialize, Validate, Into, From, Clone)]
pub struct TokenConfigByChainConfigs(pub HashMap<u32, ChainSpecificTokenConfig>);

impl ValidateUniqueItems for TokenConfigByChainConfigs {
    fn validate_unique_items(&self) -> Result<(), UniqueItemsError> {
        self.keys().cloned().collect::<Vec<_>>().validate_unique_items()
    }
}

impl Deref for TokenConfigByChainConfigs {
    type Target = HashMap<u32, ChainSpecificTokenConfig>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Deserialize, Validate, Clone)]
pub struct ChainSpecificTokenConfig {
    // The number of decimals the token has
    #[validate(minimum = 1)]
    #[validate(maximum = 18)]
    pub decimals: u8,
    // The token address on the chain
    #[validate(pattern = r"0x[a-fA-F0-9]{40}")]
    pub address: String,
    // Whether the token is supported on this chain
    pub is_enabled: bool,
}

#[derive(Debug, Deserialize, Validate)]
pub struct BungeeConfig {
    // The base URL of the Bungee API
    #[validate(
        pattern = r"https?:\/\/(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)"
    )]
    pub base_url: String,
    // The API key to access the Bungee API
    #[validate(min_length = 1)]
    pub api_key: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct CoinGeckoConfig {
    // The base URL of the CoinGecko API
    #[validate(
        pattern = r"https?:\/\/(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)"
    )]
    pub base_url: String,

    // API key to access the CoinGecko API
    #[validate(min_length = 1)]
    pub api_key: String,

    // The expiry time of the CoinGecko API key
    #[validate(minimum = 1)]
    pub expiry_sec: u64,
}

#[derive(Debug, Deserialize, Validate)]
pub struct CovalentConfig {
    // The base URL of the CoinGecko API
    #[validate(
        pattern = r"https?:\/\/(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)"
    )]
    pub base_url: String,

    // The API key to access the Covalent API
    #[validate(min_length = 1)]
    pub api_key: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct InfraConfig {
    // The URL of the Redis
    #[validate(pattern = r"redis://[-a-zA-Z0-9@:%._\+~#=]{1,256}")]
    pub redis_url: String,
    // The URL of the MongoDB
    #[validate(pattern = r"mongodb://[-a-zA-Z0-9@:%._\+~#=]{1,256}")]
    pub mongo_url: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct ServerConfig {
    // The port the server will listen on
    #[validate(minimum = 1)]
    pub port: u16,

    // The host the server will listen on
    #[validate(min_length = 1)]
    pub host: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct IndexerConfig {
    #[validate(min_length = 1)]
    pub indexer_update_topic: String,

    #[validate(min_length = 1)]
    pub indexer_update_message: String,

    #[validate(minimum = 2)]
    pub points_per_bucket: u64,
}

#[derive(Debug, Deserialize, Validate)]
pub struct SolverConfig {
    #[validate(minimum = 1.0)]
    pub x_value: f64,
    #[validate(minimum = 1.0)]
    pub y_value: f64,
}

pub fn get_sample_config() -> Config {
    Config::from_file("../../config.yaml.example").unwrap()
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, ConfigError};
    use crate::get_sample_config;

    #[test]
    fn test_config_parsing() {
        get_sample_config();
    }

    #[test]
    fn test_should_not_allow_duplicate_chains() {
        let config = r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
    covalent_name: eth-mainnet
  - id: 1
    name: Ethereum
    is_enabled: true
    covalent_name: eth-mainnet
tokens:
buckets:
bungee:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
covalent:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
coingecko:
    base_url: 'https://api.coingecko.com'
    api_key: 'my-api'
    expiry_sec: 5
infra:
    redis_url: 'redis://localhost:6379'
    mongo_url: 'mongodb://localhost:27017'
server:
    port: 8080
    host: 'localhost'
indexer_config:
    is_indexer: true
    indexer_update_topic: indexer_update
    indexer_update_message: message
    points_per_bucket: 10
solver_config:
  x_value: 2.0
  y_value: 1.0
"#;
        assert_eq!(
            if let ConfigError::SerdeError(err) = Config::from_yaml_str(&config).unwrap_err() {
                let err = err.as_validation_errors().unwrap().to_string();
                let expected_err = "{\"errors\":[],\"properties\":{\"chains\":{\"errors\":[\"The items must be unique.\"]}}}";

                err == expected_err
            } else {
                false
            },
            true
        );
    }

    #[test]
    fn test_should_not_allow_duplicate_tokens() {
        let config = r#"
chains:
tokens:
  - symbol: ETH
    is_enabled: true
    coingecko_symbol: ethereum
    by_chain:
      1:
        is_enabled: true
        decimals: 18
        address: '0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE'
      56:
        is_enabled: true
        decimals: 18
        address: '0x2170Ed0880ac9A755fd29B2688956BD959F933F8'
  - symbol: ETH
    is_enabled: true
    coingecko_symbol: ethereum
    by_chain:
      1:
        is_enabled: true
        decimals: 18
        address: '0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE'
      56:
        is_enabled: true
        decimals: 18
        address: '0x2170Ed0880ac9A755fd29B2688956BD959F933F8'
buckets:
bungee:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
covalent:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
coingecko:
    base_url: 'https://api.coingecko.com'
    api_key: 'my-api'
    expiry_sec: 5
infra:
    redis_url: 'redis://localhost:6379'
    mongo_url: 'mongodb://localhost:27017'
server:
    port: 8080
    host: 'localhost'
indexer_config:
    is_indexer: true
    indexer_update_topic: indexer_update
    indexer_update_message: message
    points_per_bucket: 10
solver_config:
  x_value: 2.0
  y_value: 1.0
"#;

        assert_eq!(
            if let ConfigError::SerdeError(err) = Config::from_yaml_str(&config).unwrap_err() {
                let err = err.as_validation_errors().unwrap().to_string();
                let expected_err = "{\"errors\":[],\"properties\":{\"tokens\":{\"errors\":[\"The items must be unique.\"]}}}";

                err == expected_err
            } else {
                false
            },
            true
        );
    }
}
