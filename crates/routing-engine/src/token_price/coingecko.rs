use std::fmt::Debug;
use std::num::ParseFloatError;

use derive_more::Display;
use reqwest::{header, StatusCode};
use serde::Deserialize;
use thiserror::Error;

use storage::KeyValueStore;

use crate::token_price::coingecko::CoingeckoClientError::RequestFailed;
use crate::token_price::TokenPriceProvider;

#[derive(Debug)]
pub struct CoingeckoClient<'config, KVStore: KeyValueStore> {
    base_url: &'config String,
    client: reqwest::Client,
    cache: &'config KVStore,
}

impl<'config, KVStore: KeyValueStore> CoingeckoClient<'config, KVStore> {
    pub fn new(
        base_url: &'config String,
        api_key: &'config String,
        cache: &'config KVStore,
    ) -> CoingeckoClient<'config, KVStore> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "x-cg-pro-api-key",
            header::HeaderValue::from_str(api_key)
                .expect("Error while building header value Invalid CoinGecko API Key"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to build reqwest client for Coingecko Client");

        CoingeckoClient { base_url, client, cache }
    }

    async fn get_fresh_token_price(
        &self,
        token_symbol: &String,
    ) -> Result<f64, CoingeckoClientError<KVStore>> {
        let response =
            self.client.get(format!("{}/coins/{}", self.base_url, token_symbol)).send().await?;

        if response.status() != StatusCode::OK {
            return Err(RequestFailed(response.status()));
        }

        let raw_text = response.text().await?;

        let response: CoinsIdResponse = serde_json::from_str(&raw_text)
            .map_err(|err| CoingeckoClientError::DeserialisationError(raw_text, err))?;

        Ok(response.market_data.current_price.usd)
    }
}

impl<'config, KVStore: KeyValueStore> TokenPriceProvider for CoingeckoClient<'config, KVStore> {
    type Error = CoingeckoClientError<KVStore>;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error> {
        let key = format!("{}_price", token_symbol);
        match self.cache.get(&key).await {
            Ok(result) => {
                let price: f64 = result.parse()?;
                if price.is_nan() {
                    Err(Self::Error::InvalidPriceReturnedFromCacheResult(result.clone()))?;
                }
                Ok(price)
            }
            Err(_) => {
                let price = self.get_fresh_token_price(token_symbol).await?;
                self.cache
                    .set(&key, &price.to_string())
                    .await
                    .map_err(CoingeckoClientError::UpdateTokenCacheError)?;
                Ok(price)
            }
        }
    }
}

#[derive(Debug, Error, Display)]
pub enum CoingeckoClientError<KVStore: KeyValueStore> {
    UpdateTokenCacheError(KVStore::Error),

    InvalidPriceReturnedFromCacheResult(String),

    InvalidPriceReturnedFromCache(#[from] ParseFloatError),

    #[display("Deserialization Error - Original String {}, Error {}", _0, _1)]
    DeserialisationError(String, serde_json::Error),

    RequestFailed(StatusCode),

    ApiCallError(#[from] reqwest::Error),
}

#[derive(Debug, Deserialize)]
struct CoinsIdResponse {
    market_data: CoinsIdResponseMarketData,
}

#[derive(Debug, Deserialize)]
struct CoinsIdResponseMarketData {
    current_price: CoinsIdResponseMarketDataCurrentPrice,
}

#[derive(Debug, Deserialize)]
struct CoinsIdResponseMarketDataCurrentPrice {
    usd: f64,
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::env;
    use std::fmt::Debug;

    use derive_more::Display;
    use thiserror::Error;

    use config::Config;
    use storage::KeyValueStore;

    use crate::CoingeckoClient;
    use crate::token_price::TokenPriceProvider;

    #[derive(Error, Debug, Display)]
    struct Err;

    #[derive(Default, Debug)]
    struct KVStore {
        map: RefCell<HashMap<String, String>>,
    }

    impl KeyValueStore for KVStore {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            match self.map.borrow().get(k) {
                Some(v) => Ok(v.clone()),
                None => Result::Err(Err),
            }
        }

        async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            todo!()
        }

        async fn set(&self, k: &String, v: &String) -> Result<(), Self::Error> {
            self.map
                .borrow_mut()
                .insert((*k.clone()).parse().unwrap(), (*v.clone()).parse().unwrap());
            Ok(())
        }

        async fn set_multiple(&self, kv: &Vec<(String, String)>) -> Result<(), Self::Error> {
            todo!()
        }
    }

    fn setup_config<'a>() -> Config {
        // let config = config::Config::from_file("../../config.yaml").unwrap();
        Config::from_yaml_str(
            r#"
chains:
  - id: 1
    name: Ethereum
    is_enabled: true
  - id: 42161
    name: Arbitrum
    is_enabled: true
tokens:
  - symbol: USDC
    is_enabled: true
    coingecko_symbol: usd-coin
    by_chain:
      1:
        is_enabled: true
        decimals: 6
        address: '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'
      42161:
        is_enabled: true
        decimals: 6
        address: '0xaf88d065e77c8cC2239327C5EDb3A432268e5831'
buckets:
bungee:
  base_url: https://api.socket.tech/v2
  api_key: <REDACTED>
covalent:
  base_url: 'https://api.bungee.exchange'
  api_key: 'my-api'
coingecko:
  base_url: 'https://api.coingecko.com/api/v3'
  api_key: 'my-api'
infra:
  redis_url: 'redis://localhost:6379'
  rabbitmq_url: 'amqp://localhost:5672'
  mongo_url: 'mongodb://localhost:27017'
server:
  port: 8080
  host: 'localhost'
indexer_config:
    is_indexer: true
    indexer_update_topic: indexer_update
    indexer_update_message: message
        "#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_should_fetch_fresh_api_price() {
        let config = setup_config();

        let api_key = env::var("COINGECKO_API_KEY").unwrap();

        let store = KVStore::default();

        let client = CoingeckoClient::new(&config.coingecko.base_url, &api_key, &store);
        let price = client.get_fresh_token_price(&"usd-coin".to_string()).await.unwrap();

        assert!(price > 0.0);
    }

    #[tokio::test]
    async fn test_should_cache_api_prices() {
        let config = setup_config();

        let api_key = env::var("COINGECKO_API_KEY").unwrap();

        let store = KVStore::default();

        let client = CoingeckoClient::new(&config.coingecko.base_url, &api_key, &store);
        let price = client.get_token_price(&"usd-coin".to_string()).await.unwrap();

        assert!(price > 0.0);
        let key = "usd-coin_price".to_string();
        assert_eq!(store.get(&key).await.unwrap().parse::<f64>().unwrap(), price);

        let price2 = client.get_token_price(&"usd-coin".to_string()).await.unwrap();
        assert_eq!(price, price2);

        store.set(&key, &"1.1".to_string()).await.unwrap();

        let price = client.get_token_price(&"usd-coin".to_string()).await.unwrap();
        assert_eq!(price, 1.1);
    }
}
