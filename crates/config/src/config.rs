use std::collections::HashMap;
use std::ops::Deref;

use derive_more::{Display, From, Into};
use serde::Deserialize;
use serde_valid::{UniqueItemsError, Validate, ValidateUniqueItems};
use serde_valid::yaml::FromYamlStr;

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
    // Whether to run the routes indexer + algorithm on this node.
    pub is_indexer: bool,
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
            is_indexer: raw_config.is_indexer,
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
            .map(|b| (b.from_chain_id, b.to_chain_id))
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
    pub is_indexer: bool,
}

#[derive(Debug, Deserialize, Validate)]
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

#[derive(Debug, Deserialize, Validate)]
pub struct ChainConfig {
    // The chain id
    #[validate(minimum = 1)]
    pub id: u32,
    // The name of the chain
    #[validate(min_length = 1)]
    pub name: String,
    // If the chain is enabled or now
    pub is_enabled: bool,
}

#[derive(Debug, Deserialize, Validate)]
pub struct TokenConfig {
    // The token symbol
    #[validate(min_length = 1)]
    pub symbol: String,
    // Whether the token across chains is supported
    pub is_enabled: bool,
    // Chain Specific Configuration
    #[validate(unique_items)]
    pub by_chain: TokenConfigByChainConfigs,
}

#[derive(Debug, Deserialize, Validate, Into, From)]
pub struct TokenConfigByChainConfigs(HashMap<u32, ChainSpecificTokenConfig>);

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

#[derive(Debug, Deserialize, Validate)]
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
    // The URL of the RabbitMQ
    #[validate(pattern = r"amqp://[-a-zA-Z0-9@:%._\+~#=]{1,256}")]
    pub rabbitmq_url: String,
    // The URL of the MongoDB
    #[validate(pattern = r"mongodb://[-a-zA-Z0-9@:%._\+~#=]{1,256}")]
    pub mongo_url: String,
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, ConfigError};

    #[test]
    fn test_config_parsing() {
        let config = r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
  - id: 56
    name: Binance Smart Chain
    is_enabled: true
tokens:
  - symbol: ETH
    is_enabled: true
    by_chain:
      1:
        is_enabled: true
        decimals: 18
        address: '0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE'
      56:
        is_enabled: true
        decimals: 18
        address: '0x2170Ed0880ac9A755fd29B2688956BD959F933F8'
  - symbol: BNB
    is_enabled: true
    by_chain:
      1:
        is_enabled: false
        decimals: 18
        address: '0xB8c77482e45F1F44dE1745F52C74426C631bDD52'
      56:
        is_enabled: true
        decimals: 18
        address: '0xEEeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'
buckets:
  - from_chain_id: 1
    to_chain_id: 56
    from_token: ETH
    to_token: BNB
    is_smart_contract_deposit_supported: true
    token_amount_from_usd: 0.1
    token_amount_to_usd: 100
bungee:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
covalent:
    base_url: 'https://api.bungee.exchange'
    api_key: 'my-api'
coingecko:
    base_url: 'https://api.coingecko.com'
infra:
    redis_url: 'redis://localhost:6379'
    rabbitmq_url: 'amqp://localhost:5672'
    mongo_url: 'mongodb://localhost:27017'
is_indexer: true
        "#;
        let config = Config::from_yaml_str(&config).unwrap();

        assert_eq!(config.chains.len(), 2);
        assert_eq!(config.chains[&1].id, 1);
        assert_eq!(config.chains[&1].name, "Ethereum");
        assert_eq!(config.chains[&1].is_enabled, true);
        assert_eq!(config.chains[&56].id, 56);
        assert_eq!(config.chains[&56].name, "Binance Smart Chain");
        assert_eq!(config.chains[&56].is_enabled, true);

        assert_eq!(config.tokens.len(), 2);
        assert_eq!(config.tokens["ETH"].symbol, "ETH");
        assert_eq!(config.tokens["ETH"].is_enabled, true);
        assert_eq!(config.tokens["ETH"].by_chain.len(), 2);
        assert_eq!(config.tokens["ETH"].by_chain[&1].decimals, 18);
        assert_eq!(
            config.tokens["ETH"].by_chain[&1].address,
            "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"
        );
        assert_eq!(config.tokens["ETH"].by_chain[&1].is_enabled, true);
        assert_eq!(config.tokens["ETH"].by_chain[&56].decimals, 18);
        assert_eq!(
            config.tokens["ETH"].by_chain[&56].address,
            "0x2170Ed0880ac9A755fd29B2688956BD959F933F8"
        );
        assert_eq!(config.tokens["ETH"].by_chain[&56].is_enabled, true);
        assert_eq!(config.tokens["BNB"].symbol, "BNB");
        assert_eq!(config.tokens["BNB"].is_enabled, true);
        assert_eq!(config.tokens["BNB"].by_chain.len(), 2);
        assert_eq!(config.tokens["BNB"].by_chain[&1].decimals, 18);
        assert_eq!(
            config.tokens["BNB"].by_chain[&1].address,
            "0xB8c77482e45F1F44dE1745F52C74426C631bDD52"
        );
        assert_eq!(config.tokens["BNB"].by_chain[&1].is_enabled, false);
        assert_eq!(config.tokens["BNB"].by_chain[&56].decimals, 18);
        assert_eq!(
            config.tokens["BNB"].by_chain[&56].address,
            "0xEEeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        );
        assert_eq!(config.tokens["BNB"].by_chain[&56].is_enabled, true);

        assert_eq!(config.buckets.len(), 1);
        assert_eq!(config.buckets[0].from_chain_id, 1);
        assert_eq!(config.buckets[0].to_chain_id, 56);
        assert_eq!(config.buckets[0].from_token, "ETH");
        assert_eq!(config.buckets[0].to_token, "BNB");
        assert_eq!(config.buckets[0].is_smart_contract_deposit_supported, true);
        assert_eq!(config.buckets[0].token_amount_from_usd, 0.1);
        assert_eq!(config.buckets[0].token_amount_to_usd, 100.0);

        assert_eq!(config.covalent.base_url, "https://api.bungee.exchange");
        assert_eq!(config.coingecko.base_url, "https://api.coingecko.com");
        assert_eq!(config.infra.redis_url, "redis://localhost:6379");
        assert_eq!(config.infra.rabbitmq_url, "amqp://localhost:5672");
        assert_eq!(config.infra.mongo_url, "mongodb://localhost:27017");
        assert_eq!(config.is_indexer, true);
    }

    #[test]
    fn test_should_not_allow_duplicate_chains() {
        let config = r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
  - id: 1
    name: Ethereum
    is_enabled: true
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
infra:
    redis_url: 'redis://localhost:6379'
    rabbitmq_url: 'amqp://localhost:5672'
    mongo_url: 'mongodb://localhost:27017'
is_indexer: true
        "#;

        assert_eq!(
            if let ConfigError::SerdeError(_) = Config::from_yaml_str(&config).unwrap_err() {
                true
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
infra:
    redis_url: 'redis://localhost:6379'
    rabbitmq_url: 'amqp://localhost:5672'
    mongo_url: 'mongodb://localhost:27017'
is_indexer: true
        "#;

        assert_eq!(
            if let ConfigError::SerdeError(_) = Config::from_yaml_str(&config).unwrap_err() {
                true
            } else {
                false
            },
            true
        );
    }
}
