use std::fmt::Display;

use derive_more::{Display, From};
use ruint;
use ruint::aliases::U256;
use ruint::Uint;

use crate::config;
use crate::token_price::TokenPriceProvider;

pub async fn get_token_amount_from_value_in_usd<'a, T: TokenPriceProvider>(
    config: &'a config::Config,
    token_price_provider: &'a T,
    token_symbol: &'a String,
    chain_id: u32,
    value_in_usd: f64,
) -> Result<U256, Errors<'a, T::Error>> {
    let token_price = token_price_provider.get_token_price(token_symbol).await?;

    let token_config = config.tokens.get(token_symbol);
    if token_config.is_none() {
        return Err(Errors::TokenConfigurationNotFound(token_symbol));
    }
    let token_config = token_config.unwrap();

    let token_config_by_chain = token_config.by_chain.get(&chain_id);
    if token_config_by_chain.is_none() {
        return Err(Errors::TokenConfigurationNotFoundForChain(token_symbol, chain_id));
    }
    let token_config_by_chain = token_config_by_chain.unwrap();

    const MULTIPLIER: f64 = 10000000.0;
    let token_amount_in_wei: U256 = Uint::from(value_in_usd * MULTIPLIER)
        * Uint::from(10).pow(Uint::from(token_config_by_chain.decimals))
        / Uint::from(token_price * MULTIPLIER);

    Ok(token_amount_in_wei)
}

#[derive(Debug, Display, From)]
pub(crate) enum Errors<'a, T: Display> {
    #[display(fmt = "Token price provider error: {}", _0)]
    TokenPriceProviderError(T),

    #[display(fmt = "Could not find token configuration for {}", _0)]
    #[from(ignore)]
    TokenConfigurationNotFound(&'a String),

    #[display(fmt = "Could not find token configuration for {} on chain {}", _0, _1)]
    #[from(ignore)]
    TokenConfigurationNotFoundForChain(&'a String, u32),
}

#[cfg(test)]
mod tests {
    use ruint::Uint;

    use config::Config;

    use crate::token_price::TokenPriceProvider;

    fn setup() -> Config {
        config::Config::from_yaml_str(
            r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
tokens:
  - symbol: USDC
    is_enabled: true
    by_chain:
      1:
        is_enabled: true
        decimals: 6
        address: '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'
buckets:
bungee:
  base_url: https://api.socket.tech/v2
  api_key: 72a5b4b0-e727-48be-8aa1-5da9d62fe635
covalent:
  base_url: 'https://api.bungee.exchange'
  api_key: 'my-api'
coingecko:
  base_url: 'https://api.coingecko.com'
  api_key: 'my-api'
infra:
  redis_url: 'redis://localhost:6379'
  rabbitmq_url: 'amqp://localhost:5672'
  mongo_url: 'mongodb://localhost:27017'
server:
  port: 8080
  host: 'localhost'
is_indexer: true
        "#,
        )
        .unwrap()
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;

    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = String;

        async fn get_token_price(&self, _: &String) -> Result<f64, Self::Error> {
            Ok(0.1)
        }
    }

    #[tokio::test]
    async fn test_get_token_amount_from_value_in_usd() {
        let config = setup();
        let token_price_provider = TokenPriceProviderStub;

        let token_symbol = String::from("USDC");
        let chain_id = 1;
        let value_in_usd = 10.0;

        let result = super::get_token_amount_from_value_in_usd(
            &config,
            &token_price_provider,
            &token_symbol,
            chain_id,
            value_in_usd,
        )
        .await;

        assert_eq!(result.unwrap(), Uint::from(100000000));
    }
}
