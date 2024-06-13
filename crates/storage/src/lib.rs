pub trait RoutingModelStore {
    type Error;

    async fn get(&self, k: String) -> Result<String, Self::Error>;

    async fn set(&self, k: String, v: String) -> Result<(), Self::Error>;
}

pub trait MessageQueue {
    async fn publish(&self, topic: &str, message: &str) -> Result<(), String>;

    async fn subscribe(&self, topic: &str) -> Result<String, String>;
}
