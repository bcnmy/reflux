use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use log::info;
use redis::{self, aio, AsyncCommands, ControlFlow, Msg, PubSubCommands};
use redis::RedisError;
use thiserror::Error;

use crate::{KeyValueStore, MessageQueue};

#[derive(Debug, Clone)]
pub struct RedisClient {
    client: redis::Client,
    connection: aio::MultiplexedConnection,
}

impl RedisClient {
    pub async fn build(redis_url: &String) -> Result<Self, RedisClientError> {
        let client = redis::Client::open(redis_url.clone())?;
        let connection = client.get_multiplexed_async_connection().await?;
        Ok(RedisClient { client, connection })
    }
}

#[async_trait]
impl KeyValueStore for RedisClient {
    type Error = RedisClientError;

    // Todo: This should return an option
    async fn get(&self, k: &String) -> Result<String, Self::Error> {
        info!("Getting key: {}", k);
        self.connection.clone().get(k).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error> {
        info!("Getting keys: {:?}", k);
        self.connection.clone().mget(k).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn set(&self, k: &String, v: &String, duration: Duration) -> Result<(), Self::Error> {
        info!("Setting key: {} with value: {} and expiry: {}s", k, v, duration.as_secs());
        self.connection
            .clone()
            .set_ex(k, v, duration.as_secs())
            .await
            .map_err(RedisClientError::RedisLibraryError)
    }

    async fn set_multiple(&self, kv: &Vec<(String, String)>) -> Result<(), Self::Error> {
        info!("Setting keys: {:?}", kv);
        self.connection.clone().mset(kv).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn get_all_keys(&self) -> Result<Vec<String>, RedisClientError> {
        info!("Fetching all keys");
        let keys: Vec<String> = self.connection.clone().keys("*").await?;
        Ok(keys)
    }

    async fn get_all_key_values(
        &self,
        batch_size: Option<usize>,
    ) -> Result<HashMap<String, String>, RedisClientError> {
        info!("Fetching all key-value pairs");
        let keys = self.get_all_keys().await?;

        let batch_size = batch_size.unwrap_or(keys.len());
        let mut values = Vec::new();
        for batch in keys.chunks(batch_size) {
            values.extend(self.connection.clone().mget::<'_, _, Vec<_>>(batch).await?.into_iter());
        }

        let kv_pairs = keys.into_iter().zip(values.into_iter()).collect();
        Ok(kv_pairs)
    }
}

#[async_trait]
impl MessageQueue for RedisClient {
    type Error = RedisClientError;

    async fn publish(&self, topic: &str, message: &str) -> Result<(), Self::Error> {
        info!("Publishing to topic: {} with message: {}", topic, message);
        self.connection
            .clone()
            .publish(topic, message)
            .await
            .map_err(RedisClientError::RedisLibraryError)
    }

    fn subscribe<U>(
        &self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error> {
        info!("Subscribing to topic: {}", topic);
        let mut connection = self.client.get_connection()?;
        connection.subscribe(topic, callback)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RedisClientError {
    #[error("Error thrown from Redis Library: {0}")]
    RedisLibraryError(#[from] RedisError),

    #[error("Redis Mutex poisoned")]
    MutexPoisonedError,
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use tokio::{self};

    use super::*;

    async fn setup() -> RedisClient {
        let redis_url = "redis://localhost:6379".to_string();
        RedisClient::build(&redis_url).await.unwrap()
    }

    #[tokio::test]
    async fn test_key_store() {
        let client = setup().await;

        let keys = vec!["test_key1".to_string(), "test_key2".to_string()];
        let values = vec!["test_value1".to_string(), "test_value2".to_string()];

        // Clear
        client.set(&keys[0], &String::from(""), Duration::from_secs(10)).await.unwrap();

        // Test set
        client.set(&keys[0], &values[0], Duration::from_secs(10)).await.unwrap();

        // Test get
        let value = client.get(&keys[0]).await.unwrap();
        assert_eq!(value, values[0]);

        // Clear
        client.set(&keys[0], &String::from(""), Duration::from_secs(10)).await.unwrap();
        client.set(&keys[1], &String::from(""), Duration::from_secs(10)).await.unwrap();

        // Multi Set
        client
            .set_multiple(
                &keys.iter().zip(values.iter()).map(|(k, v)| (k.clone(), v.clone())).collect(),
            )
            .await
            .unwrap();

        // Multi Get
        let values = client.get_multiple(&keys).await.unwrap();
        assert_eq!(values, vec!["test_value1".to_string(), "test_value2".to_string()]);
    }

    #[tokio::test]
    async fn test_get_all_key_values() {
        let client = setup().await;

        // Set some keys
        client
            .set(&"key1".to_string(), &"value1".to_string(), Duration::from_secs(60))
            .await
            .unwrap();
        client
            .set(&"key2".to_string(), &"value2".to_string(), Duration::from_secs(60))
            .await
            .unwrap();
        client
            .set(&"key3".to_string(), &"value3".to_string(), Duration::from_secs(60))
            .await
            .unwrap();

        // Fetch all key-values
        let key_values = client.get_all_key_values(None).await.unwrap();

        assert_eq!(key_values.get("key1").unwrap(), "value1");
        assert_eq!(key_values.get("key2").unwrap(), "value2");
        assert_eq!(key_values.get("key3").unwrap(), "value3");
    }

    #[tokio::test]
    async fn test_pub_sub() {
        let (tx, rx) = channel::<String>();
        let client = setup().await;

        tokio::task::spawn_blocking(move || {
            client
                .subscribe("TOPIC", |msg: Msg| -> ControlFlow<String> {
                    let message = msg.get_payload().unwrap();
                    tx.send(message).expect("Sending message failed");

                    ControlFlow::Break("DONE".to_string())
                })
                .unwrap();
        });

        tokio::time::sleep(Duration::from_secs(5)).await;

        let client = setup().await;
        client.publish("TOPIC", "HELLO").await.unwrap();

        loop {
            if let Ok(data) = rx.recv() {
                assert_eq!(data, "HELLO".to_string());
                break;
            }
        }
    }
}
