use crate::db_provider::DBProvider;
use crate::errors::DBError;
use async_trait::async_trait;
use derive_more::Display;
use mongodb::{
    bson::{self, doc, Document},
    options::IndexOptions,
    Client, Collection,
};
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Display, Clone)]
#[display(
    "MongoDBProvider {{ client: {:?}, db_name: {}, collection_name: {} }}",
    client,
    db_name,
    collection_name
)]
pub struct MongoDBProvider {
    pub client: Client,
    db_name: String,
    collection_name: String,
}

impl MongoDBProvider {
    pub async fn new(
        mongodb_uri: &str,
        db_name: String,
        collection_name: String,
        create_indexes: bool,
    ) -> Result<Self, DBError> {
        let client = mongodb::Client::with_uri_str(mongodb_uri).await?;
        let provider = Self { client, db_name, collection_name };
        if create_indexes {
            provider.create_indexes().await?;
        }
        Ok(provider)
    }

    pub fn get_collection(&self) -> Collection<Document> {
        self.client.database(&self.db_name).collection(&self.collection_name)
    }

    async fn create_indexes(&self) -> Result<(), DBError> {
        let collection: Collection<Document> = self.get_collection();
        let model = mongodb::IndexModel::builder()
            .keys(doc! { "user_id": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        collection.create_index(model, None).await?;
        Ok(())
    }

    pub fn to_document<T: Serialize>(&self, item: &T) -> Result<Document, DBError> {
        let doc = bson::to_bson(item)?
            .as_document()
            .cloned()
            .ok_or_else(|| DBError::Other("Failed to convert item to BSON document".to_string()))?;
        Ok(doc)
    }

    pub fn from_document<T: DeserializeOwned>(&self, doc: Document) -> Result<T, DBError> {
        let item = bson::from_bson(bson::Bson::Document(doc))?;
        Ok(item)
    }
}

#[async_trait]
impl DBProvider for MongoDBProvider {
    async fn create(&self, item: &Document) -> Result<(), DBError> {
        let collection = self.get_collection();
        collection.insert_one(item.clone(), None).await?;
        Ok(())
    }

    async fn read(&self, query: &Document) -> Result<Option<Document>, DBError> {
        let collection = self.get_collection();
        let result = collection.find_one(query.clone(), None).await?;
        Ok(result)
    }

    async fn update(&self, query: &Document, update: &Document) -> Result<(), DBError> {
        let collection = self.get_collection();
        let update_doc = doc! { "$set": update.clone() };
        collection.update_one(query.clone(), update_doc, None).await?;
        Ok(())
    }

    async fn delete(&self, query: &Document) -> Result<(), DBError> {
        let collection = self.get_collection();
        collection.delete_one(query.clone(), None).await?;
        Ok(())
    }
}
