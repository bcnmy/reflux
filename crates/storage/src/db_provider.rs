use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::error::Error;

#[async_trait]
pub trait DBProvider {
    async fn create<T: Serialize + Send + Sync>(&self, item: &T) -> Result<(), Box<dyn Error>>;
    async fn read<T: for<'de> Deserialize<'de> + Unpin + Send + Sync>(&self, query: &mongodb::bson::Document) -> Result<Option<T>, Box<dyn Error>>;
    async fn update<T: Serialize + Send + Sync>(&self, query: &mongodb::bson::Document, update: &T) -> Result<(), Box<dyn Error>>;
    async fn delete(&self, query: &mongodb::bson::Document) -> Result<(), Box<dyn Error>>;
}
