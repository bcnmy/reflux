use ruint;
use ruint::aliases::U256;
use ruint::Uint;
use std::fmt::Debug;
use thiserror::Error;
use crate::token_price::TokenPriceProvider;

pub async fn get_token_amount_from_value_in_usd<'config, T: TokenPriceProvider>(
    config: &'config config::Config,
    token_price_provider: &'config T,
    token_symbol: &'config String,
    chain_id: u32,
    value_in_usd: &f64,
) -> Result<U256, Errors<T::Error>> {
    let token_price = get_token_price(config, token_price_provider, token_symbol).await?;

    let token_config = config.tokens.get(token_symbol);
    if token_config.is_none() {
        return Err(Errors::TokenConfigurationNotFound(token_symbol.clone()));
    }
    let token_config = token_config.unwrap();

    let token_config_by_chain = token_config.by_chain.get(&chain_id);
    if token_config_by_chain.is_none() {
        return Err(Errors::TokenConfigurationNotFoundForChain(token_symbol.clone(), chain_id));
    }
    let token_config_by_chain = token_config_by_chain.unwrap();

    const MULTIPLIER: f64 = 10000000.0;
    let token_amount_in_wei: U256 = Uint::from(value_in_usd * MULTIPLIER)
        * Uint::from(10).pow(Uint::from(token_config_by_chain.decimals))
        / Uint::from(token_price * MULTIPLIER);

    Ok(token_amount_in_wei)
}

pub async fn get_token_price<'config, T: TokenPriceProvider>(
    config: &'config config::Config,
    token_price_provider: &'config T,
    token_symbol: &'config String,
) -> Result<f64, Errors<T::Error>> {
    let token_config = config.tokens.get(token_symbol);
    if token_config.is_none() {
        return Err(Errors::TokenConfigurationNotFound(token_symbol.clone()));
    }
    let token_config = token_config.unwrap();

    let token_price = token_price_provider
        .get_token_price(&token_config.coingecko_symbol)
        .await
        .map_err(Errors::<T::Error>::TokenPriceProviderError)?;

    return Ok(token_price);
}

#[derive(Debug, Error)]
pub enum Errors<T: Debug> {
    #[error("Token price provider error: {:?}", _0)]
    TokenPriceProviderError(#[from] T),

    #[error("Could not find token configuration for {}", _0)]
    TokenConfigurationNotFound(String),

    #[error("Could not find token configuration for {} on chain {}", _0, _1)]
    TokenConfigurationNotFoundForChain(String, u32),
}

#[cfg(test)]
mod tests {
    use std::fmt::Error;

    use ruint::Uint;

    use config::{get_sample_config, Config};

    use crate::token_price::TokenPriceProvider;

    fn setup() -> Config {
        get_sample_config()
    }

    #[derive(Debug)]
    struct TokenPriceProviderStub;

    impl TokenPriceProvider for TokenPriceProviderStub {
        type Error = Error;

        async fn get_token_price(&self, _: &String) -> Result<f64, Self::Error> {
            Ok(0.1)
        }
    }

    #[tokio::test]
    async fn test_get_token_amount_from_value_in_usd() {
        let config = setup();
        let mut token_price_provider = TokenPriceProviderStub;

        let token_symbol = String::from("USDC");
        let chain_id = 1;
        let value_in_usd = 10.0;

        let result = super::get_token_amount_from_value_in_usd(
            &config,
            &mut token_price_provider,
            &token_symbol,
            chain_id,
            &value_in_usd,
        )
        .await;

        assert_eq!(result.unwrap(), Uint::from(100000000));
    }
}
