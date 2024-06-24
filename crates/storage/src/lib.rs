use std::error::Error;
use std::fmt::Debug;
use std::time::Duration;

pub use ::redis::{ControlFlow, Msg};

pub use redis::RedisClient;

pub mod db_provider;
pub mod errors;
pub mod mongodb_provider;

mod redis;

pub trait KeyValueStore: Debug {
    type Error: Error + Debug;

    async fn get(&self, k: &String) -> Result<String, Self::Error>;

    async fn get_multiple(&self, k: &Vec<String>) -> Result<Vec<String>, Self::Error>;

    async fn set(&self, k: &String, v: &String, expiry: Duration) -> Result<(), Self::Error>;

    async fn set_multiple(&self, kv: &Vec<(String, String)>) -> Result<(), Self::Error>;
}

pub trait MessageQueue: Debug {
    type Error: Error + Debug;

    async fn publish(&self, topic: &str, message: &str) -> Result<(), Self::Error>;

    fn subscribe<U>(
        &self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error>;
}
