use std::collections::HashMap;
use std::error::Error;
use std::fmt::Debug;
use std::time::Duration;

pub use ::redis::{ControlFlow, Msg};
use async_trait::async_trait;
use mongodb::bson::Document;

pub use redis_client::{RedisClient, RedisClientError};

pub mod mongodb_client;

mod redis_client;

#[async_trait]
pub trait KeyValueStore: Debug + Send + Sync {
    type Error: Error + Debug + Send + Sync;

    async fn get(&self, k: &String) -> Result<String, Self::Error>;

    async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error>;

    async fn set(&self, k: &String, v: &String, expiry: Duration) -> Result<(), Self::Error>;

    async fn set_multiple(&self, kv: &Vec<(String, String)>) -> Result<(), Self::Error>;

    async fn get_all_keys(&self) -> Result<Vec<String>, RedisClientError>;

    async fn get_all_key_values(
        &self,
        batch_size: Option<usize>,
    ) -> Result<HashMap<String, String>, RedisClientError>;
}

#[async_trait]
pub trait MessageQueue: Debug + Send + Sync {
    type Error: Error + Debug + Send + Sync;

    async fn publish(&self, topic: &str, message: &str) -> Result<(), Self::Error>;

    fn subscribe<U>(
        &self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error>;
}

#[async_trait]
pub trait DBProvider: Debug + Send + Sync {
    type Error: Error + Debug + Send + Sync;

    async fn create(&self, item: &Document) -> Result<(), Self::Error>;

    async fn read(&self, query: &Document) -> Result<Option<Document>, Self::Error>;

    async fn update(&self, query: &Document, update: &Document) -> Result<(), Self::Error>;

    async fn delete(&self, query: &Document) -> Result<(), Self::Error>;
}
