use std::collections::HashMap;
use std::error::Error;
use std::fmt::Debug;
use std::future;
use std::time::Duration;

pub use ::redis::{ControlFlow, Msg};
use mongodb::bson::Document;

pub use redis_client::{RedisClient, RedisClientError};

pub mod mongodb_client;

mod redis_client;

pub trait KeyValueStore: Debug {
    type Error: Error + Debug;

    fn get(&self, k: &String) -> impl future::Future<Output = Result<String, Self::Error>>;

    fn get_multiple(
        &self,
        k: &Vec<String>,
    ) -> impl future::Future<Output = Result<Vec<String>, Self::Error>>;

    fn set(
        &self,
        k: &String,
        v: &String,
        expiry: Duration,
    ) -> impl future::Future<Output = Result<(), Self::Error>>;

    fn set_multiple(
        &self,
        kv: &Vec<(String, String)>,
    ) -> impl future::Future<Output = Result<(), Self::Error>>;

    fn get_all_keys(&self) -> impl future::Future<Output = Result<Vec<String>, RedisClientError>>;

    fn get_all_key_values(
        &self,
    ) -> impl future::Future<Output = Result<HashMap<String, String>, RedisClientError>>;
}

pub trait MessageQueue: Debug {
    type Error: Error + Debug;

    fn publish(
        &self,
        topic: &str,
        message: &str,
    ) -> impl future::Future<Output = Result<(), Self::Error>>;

    fn subscribe<U>(
        &self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error>;
}

pub trait DBProvider: Debug {
    type Error: Error + Debug;

    fn create(&self, item: &Document) -> impl future::Future<Output = Result<(), Self::Error>>;

    fn read(
        &self,
        query: &Document,
    ) -> impl future::Future<Output = Result<Option<Document>, Self::Error>>;

    fn update(
        &self,
        query: &Document,
        update: &Document,
    ) -> impl future::Future<Output = Result<(), Self::Error>>;

    fn delete(&self, query: &Document) -> impl future::Future<Output = Result<(), Self::Error>>;
}
