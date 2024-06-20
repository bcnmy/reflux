use redis;
use redis::RedisError;
use redis::{aio, AsyncCommands, Commands, ControlFlow, Msg, PubSubCommands};
use thiserror::Error;

use config;

use crate::{MessageQueue, RoutingModelStore};

struct RedisClient {
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

impl RoutingModelStore for RedisClient {
    type Error = RedisClientError;

    async fn get(&mut self, k: &String) -> Result<String, Self::Error> {
        self.connection.get(k).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn get_multiple(&mut self, k: &Vec<String>) -> Result<Vec<String>, Self::Error> {
        self.connection.mget(k).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn set(&mut self, k: &String, v: &String) -> Result<(), Self::Error> {
        self.connection.set(k, v).await.map_err(RedisClientError::RedisLibraryError)
    }

    async fn set_multiple(&mut self, kv: &Vec<(String, String)>) -> Result<(), Self::Error> {
        self.connection.mset(kv).await.map_err(RedisClientError::RedisLibraryError)
    }
}

impl MessageQueue for RedisClient {
    type Error = RedisClientError;

    async fn publish(&mut self, topic: &str, message: &str) -> Result<(), Self::Error> {
        self.connection.publish(topic, message).await.map_err(RedisClientError::RedisLibraryError)
    }

    fn subscribe<U>(
        &mut self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error> {
        let mut connection = self.client.get_connection()?;
        connection.subscribe(topic, callback)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RedisClientError {
    #[error("Error thrown from Redis Library: {0}")]
    RedisLibraryError(#[from] RedisError),
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use tokio;

    use super::*;

    async fn setup() -> RedisClient {
        let redis_url = "redis://localhost:6379".to_string();
        RedisClient::build(&redis_url).await.unwrap()
    }

    #[tokio::test]
    async fn test_key_store() {
        let mut client = setup().await;

        let keys = vec!["test_key1".to_string(), "test_key2".to_string()];
        let values = vec!["test_value1".to_string(), "test_value2".to_string()];

        // Clear
        client.set(&keys[0], &String::from("")).await.unwrap();

        // Test set
        client.set(&keys[0], &values[0]).await.unwrap();

        // Test get
        let value = client.get(&keys[0]).await.unwrap();
        assert_eq!(value, values[0]);

        // Clear
        client.set(&keys[0], &String::from("")).await.unwrap();
        client.set(&keys[1], &String::from("")).await.unwrap();

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
    async fn test_pub_sub() {
        let (tx, mut rx) = channel::<String>();
        let mut client = setup().await;

        tokio::task::spawn_blocking(move || {
            client
                .subscribe("TOPIC", |msg: Msg| -> ControlFlow<String> {
                    let message = msg.get_payload().unwrap();
                    tx.send(message).expect("Sending message failed");

                    ControlFlow::Break("DONE".to_string())
                })
                .unwrap();
        });

        let mut client = setup().await;
        client.publish("TOPIC", "HELLO").await.unwrap();

        loop {
            if let Ok(data) = rx.recv() {
                assert_eq!(data, "HELLO".to_string());
                break;
            }
        }
    }
}
