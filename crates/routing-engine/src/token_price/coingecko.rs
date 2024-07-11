use std::fmt::Debug;
use std::num::ParseFloatError;
use std::time::Duration;

use derive_more::Display;
use log::{error, info};
use reqwest::{header, StatusCode};
use serde::Deserialize;
use thiserror::Error;

use storage::KeyValueStore;

use crate::token_price::coingecko::CoingeckoClientError::RequestFailed;
use crate::token_price::TokenPriceProvider;

#[derive(Debug)]
pub struct CoingeckoClient<KVStore: KeyValueStore> {
    base_url: String,
    client: reqwest::Client,
    cache: KVStore,
    key_expiry: Duration,
}

impl<KVStore: KeyValueStore> CoingeckoClient<KVStore> {
    pub fn new(
        base_url: String,
        api_key: String,
        cache: KVStore,
        key_expiry: Duration,
    ) -> CoingeckoClient<KVStore> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "x-cg-pro-api-key",
            header::HeaderValue::from_str(&api_key)
                .expect("Error while building header value Invalid CoinGecko API Key"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to build reqwest client for Coingecko Client");

        CoingeckoClient { base_url, client, cache, key_expiry }
    }

    async fn get_fresh_token_price(
        &self,
        token_symbol: &String,
    ) -> Result<f64, CoingeckoClientError<KVStore>> {
        info!("Fetching fresh token price for {}", token_symbol);

        let response =
            self.client.get(format!("{}/coins/{}", self.base_url, token_symbol)).send().await?;

        if response.status() != StatusCode::OK {
            error!("CoinGecko /coins/ Request failed with status: {}", response.status());
            return Err(RequestFailed(response.status()));
        }

        let raw_text = response.text().await?;

        let response: CoinsIdResponse = serde_json::from_str(&raw_text)
            .map_err(|err| CoingeckoClientError::DeserialisationError(raw_text, err))?;

        let result = response.market_data.current_price.usd;

        info!("Token price fetched from API for token {}: {}", token_symbol, result);

        Ok(result)
    }
}

impl<KVStore: KeyValueStore> TokenPriceProvider for CoingeckoClient<KVStore> {
    type Error = CoingeckoClientError<KVStore>;

    async fn get_token_price(&self, token_symbol: &String) -> Result<f64, Self::Error> {
        info!("Fetching token price for {}", token_symbol);

        let key = format!("{}_price", token_symbol);
        match self.cache.get(&key).await {
            Ok(result) => {
                info!("Token price fetched from cache");

                let price: f64 = result.parse()?;
                if price.is_nan() {
                    Err(Self::Error::InvalidPriceReturnedFromCacheResult(result.clone()))?;
                }
                Ok(price)
            }
            Err(_) => {
                info!("Token price not found in cache");

                let price = self.get_fresh_token_price(token_symbol).await?;
                self.cache
                    .set(&key, &price.to_string(), self.key_expiry)
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
    use std::collections::HashMap;
    use std::env;
    use std::fmt::Debug;
    use std::sync::Mutex;
    use std::time::Duration;

    use derive_more::Display;
    use thiserror::Error;

    use config::{Config, get_sample_config};
    use storage::{KeyValueStore, RedisClientError};

    use crate::CoingeckoClient;
    use crate::token_price::TokenPriceProvider;

    #[derive(Error, Debug, Display)]
    struct Err;

    #[derive(Default, Debug)]
    struct KVStore {
        map: Mutex<HashMap<String, String>>,
    }

    impl KeyValueStore for KVStore {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            match self.map.lock().unwrap().get(k) {
                Some(v) => Ok(v.clone()),
                None => Result::Err(Err),
            }
        }

        async fn get_multiple(&self, _: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            unimplemented!()
        }

        async fn set(&self, k: &String, v: &String, _: Duration) -> Result<(), Self::Error> {
            self.map
                .lock()
                .unwrap()
                .insert((*k.clone()).parse().unwrap(), (*v.clone()).parse().unwrap());
            Ok(())
        }

        async fn set_multiple(&self, _: &Vec<(String, String)>) -> Result<(), Self::Error> {
            unimplemented!()
        }

        async fn get_all_keys(&self) -> Result<Vec<String>, RedisClientError> {
            unimplemented!()
        }

        async fn get_all_key_values(&self) -> Result<HashMap<String, String>, RedisClientError> {
            unimplemented!()
        }
    }

    fn setup_config<'a>() -> Config {
        get_sample_config()
    }

    #[tokio::test]
    async fn test_should_fetch_fresh_api_price() {
        let config = setup_config();

        let api_key = env::var("COINGECKO_API_KEY").unwrap();

        let store = KVStore::default();

        let client = CoingeckoClient::new(
            config.coingecko.base_url.clone(),
            api_key,
            store,
            Duration::from_secs(config.coingecko.expiry_sec),
        );
        let price = client.get_fresh_token_price(&"usd-coin".to_string()).await.unwrap();

        assert!(price > 0.0);
    }

    #[tokio::test]
    async fn test_should_cache_api_prices() {
        let config = setup_config();

        let api_key = env::var("COINGECKO_API_KEY").unwrap();

        let store = KVStore::default();

        let client = CoingeckoClient::new(
            config.coingecko.base_url.clone(),
            api_key,
            store,
            Duration::from_secs(config.coingecko.expiry_sec),
        );
        let price = client.get_token_price(&"usd-coin".to_string()).await.unwrap();

        assert!(price > 0.0);
        let key = "usd-coin_price".to_string();
        assert_eq!(client.cache.get(&key).await.unwrap().parse::<f64>().unwrap(), price);

        let price2 = client.get_token_price(&"usd-coin".to_string()).await.unwrap();
        assert_eq!(price, price2);

        client.cache.set(&key, &"1.1".to_string(), Duration::from_secs(10)).await.unwrap();

        let price = client.get_token_price(&"usd-coin".to_string()).await.unwrap();
        assert_eq!(price, 1.1);
    }
}
