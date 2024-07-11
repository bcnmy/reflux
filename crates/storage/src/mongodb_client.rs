use async_trait::async_trait;
use derive_more::Display;
use mongodb::{
    bson::{self, doc, Document},
    Client,
    Collection, options::IndexOptions,
};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use crate::DBProvider;

#[derive(Debug, Display, Clone)]
#[display(
    "MongoDBProvider {{ client: {:?}, db_name: {}, collection_name: {} }}",
    client,
    db_name,
    collection_name
)]
pub struct MongoDBClient {
    pub client: Client,
    db_name: String,
    collection_name: String,
}

impl MongoDBClient {
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
impl DBProvider for MongoDBClient {
    type Error = DBError;

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

#[derive(Error, Debug)]
pub enum DBError {
    #[error("MongoDB error: {0}")]
    Mongo(#[from] mongodb::error::Error),

    #[error("BSON serialization error: {0}")]
    BsonSerialization(#[from] bson::ser::Error),

    #[error("BSON deserialization error: {0}")]
    BsonDeserialization(#[from] bson::de::Error),

    #[error("Other error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use mongodb::{bson::doc, Client};
    use serde::{Deserialize, Serialize};
    use serial_test::serial;
    use tokio;
    use uuid::Uuid;

    use crate::DBProvider;
    use crate::mongodb_client::{DBError, MongoDBClient};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestUser {
        user_id: String,
        name: String,
    }

    // Global test configuration constants
    const DB_URI: &str = "mongodb://localhost:27017";
    const DB_NAME: &str = "test_db";
    const COLLECTION_NAME: &str = "test_collection";

    // Helper function to setup the MongoDBProvider
    async fn setup_db_provider(create_indexes: bool) -> Result<MongoDBClient, DBError> {
        let db_provider = MongoDBClient::new(
            DB_URI,
            DB_NAME.to_string(),
            COLLECTION_NAME.to_string(),
            create_indexes,
        )
        .await?;
        Ok(db_provider)
    }

    async fn teardown() -> Result<(), DBError> {
        let client = Client::with_uri_str(DB_URI).await?;
        client.database(DB_NAME).drop(None).await?;
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn able_to_create_db_provider() -> Result<(), DBError> {
        let _ = setup_db_provider(true).await?;
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_read() -> Result<(), DBError> {
        let db_provider = setup_db_provider(true).await?;

        let user_id = Uuid::new_v4().to_string();
        let user = TestUser { user_id: user_id.clone(), name: "Alice".to_string() };

        // Test create
        db_provider.create(&db_provider.to_document(&user)?).await?;

        // Test read
        let query = doc! { "user_id": &user.user_id };
        let result = db_provider.read(&query).await?;
        let read_user: TestUser = db_provider.from_document(result.unwrap())?;
        assert_eq!(read_user, user);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_unique_user_id() -> Result<(), DBError> {
        let db_provider = setup_db_provider(true).await?;

        let user_id = Uuid::new_v4().to_string();
        let user1 = TestUser { user_id: user_id.clone(), name: "Alice".to_string() };

        let user2 = TestUser { user_id: user_id.clone(), name: "Bob".to_string() };

        // Test create
        db_provider.create(&db_provider.to_document(&user1)?).await?;

        // Attempt to create another user with the same user_id
        let result = db_provider.create(&db_provider.to_document(&user2)?).await;

        // Ensure it fails due to unique index
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_update() -> Result<(), DBError> {
        let db_provider = setup_db_provider(true).await?;

        let user_id = Uuid::new_v4().to_string();
        let user = TestUser { user_id: user_id.clone(), name: "Alice".to_string() };

        // Test create
        db_provider.create(&db_provider.to_document(&user)?).await?;

        // Test update
        let updated_user = TestUser { user_id: user_id.clone(), name: "Bob".to_string() };
        let query = doc! { "user_id": &user.user_id };
        db_provider.update(&query, &db_provider.to_document(&updated_user)?).await?;

        // Test read after update
        let result = db_provider.read(&query).await?;
        let read_user: TestUser = db_provider.from_document(result.unwrap())?;
        assert_eq!(read_user, updated_user);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_delete() -> Result<(), DBError> {
        let db_provider = setup_db_provider(true).await?;

        let user_id = Uuid::new_v4().to_string();
        let user = TestUser { user_id: user_id.clone(), name: "Alice".to_string() };

        // Test create
        db_provider.create(&db_provider.to_document(&user)?).await?;

        // Test delete
        let query = doc! { "user_id": &user.user_id };
        db_provider.delete(&query).await?;

        // Test read after delete
        let result = db_provider.read(&query).await?;
        assert_eq!(result, None);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn teardown_db() -> Result<(), DBError> {
        teardown().await?;
        Ok(())
    }
}
