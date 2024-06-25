use std::error::Error;
use std::fmt::Debug;
use std::future;
use std::time::Duration;

pub use ::redis::{ControlFlow, Msg};

pub use redis::RedisClient;

pub mod db_provider;
pub mod errors;
pub mod mongodb_provider;

mod redis;

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
