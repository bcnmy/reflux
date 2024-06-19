use std::fmt::Debug;

pub use ::redis::{ControlFlow, Msg};

pub mod db_provider;
pub mod errors;
pub mod mongodb_provider;

mod account;
mod redis;

pub trait RoutingModelStore {
    type Error: Debug;

    async fn get(&mut self, k: &String) -> Result<String, Self::Error>;

    async fn get_multiple(&mut self, k: &Vec<String>) -> Result<Vec<String>, Self::Error>;

    async fn set(&mut self, k: &String, v: &String) -> Result<(), Self::Error>;

    async fn set_multiple(&mut self, kv: &Vec<(String, String)>) -> Result<(), Self::Error>;
}

pub trait MessageQueue {
    type Error: Debug;

    async fn publish(&mut self, topic: &str, message: &str) -> Result<(), Self::Error>;

    fn subscribe<U>(
        &mut self,
        topic: &str,
        callback: impl FnMut(Msg) -> ControlFlow<U>,
    ) -> Result<(), Self::Error>;
}
