use crate::db_provider::DBProvider;
use async_trait::async_trait;
use derive_more::Display;
use mongodb::{
    bson::{self, doc, Document},
    options::IndexOptions,
    Client, Collection,
};
use serde::{Deserialize, Serialize};
use std::error::Error;

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
        client: Client,
        db_name: String,
        collection_name: String,
        create_indexes: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let provider = Self { client, db_name, collection_name };
        if create_indexes {
            provider.create_indexes().await?;
        }
        Ok(provider)
    }

    pub fn get_collection<T>(&self) -> Collection<T> {
        self.client.database(&self.db_name).collection(&self.collection_name)
    }

    async fn create_indexes(&self) -> Result<(), Box<dyn Error>> {
        let collection: Collection<Document> = self.get_collection();
        let model = mongodb::IndexModel::builder()
            .keys(doc! { "user_id": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        collection.create_index(model, None).await?;
        Ok(())
    }
}

#[async_trait]
impl DBProvider for MongoDBProvider {
    async fn create<T: Serialize + Send + Sync>(&self, item: &T) -> Result<(), Box<dyn Error>> {
        let collection: Collection<Document> = self.get_collection();
        collection.insert_one(bson::to_bson(item)?.as_document().unwrap().clone(), None).await?;
        Ok(())
    }

    async fn read<T: for<'de> Deserialize<'de> + Unpin + Send + Sync>(
        &self,
        query: &Document,
    ) -> Result<Option<T>, Box<dyn Error>> {
        let collection: Collection<Document> = self.get_collection();
        let result = collection.find_one(query.clone(), None).await?;
        Ok(result.and_then(|doc| bson::from_bson(bson::Bson::Document(doc)).ok()))
    }

    async fn update<T: Serialize + Send + Sync>(
        &self,
        query: &Document,
        update: &T,
    ) -> Result<(), Box<dyn Error>> {
        let collection: Collection<Document> = self.get_collection();
        let update_doc = doc! { "$set": bson::to_bson(update)?.as_document().unwrap().clone() };
        collection.update_one(query.clone(), update_doc, None).await?;
        Ok(())
    }

    async fn delete(&self, query: &Document) -> Result<(), Box<dyn Error>> {
        let collection: Collection<Document> = self.get_collection();
        collection.delete_one(query.clone(), None).await?;
        Ok(())
    }
}
