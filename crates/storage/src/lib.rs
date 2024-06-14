use std::fmt::Debug;

mod redis;

pub trait RoutingModelStore {
    type Error: Debug;

    async fn get(&mut self, k: &String) -> Result<String, Self::Error>;

    async fn get_multiple(&mut self, k: &Vec<String>) -> Result<Vec<String>, Self::Error>;

    async fn set(&mut self, k: &String, v: &String) -> Result<(), Self::Error>;

    async fn set_multiple(&mut self, kv: &Vec<(String, String)>) -> Result<(), Self::Error>;
}

pub trait MessageQueue {
    async fn publish(&mut self, topic: &str, message: &str) -> Result<(), String>;

    async fn subscribe(&mut self, topic: &str) -> Result<String, String>;
}
