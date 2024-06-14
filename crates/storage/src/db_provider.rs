use async_trait::async_trait;
use mongodb::bson::Document;
use crate::errors::DBError;

#[async_trait]
pub trait DBProvider {
    async fn create(&self, item: &Document) -> Result<(), DBError>;
    async fn read(&self, query: &Document) -> Result<Option<Document>, DBError>;
    async fn update(&self, query: &Document, update: &Document) -> Result<(), DBError>;
    async fn delete(&self, query: &Document) -> Result<(), DBError>;
}
