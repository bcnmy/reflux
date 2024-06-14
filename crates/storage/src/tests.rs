use mongodb::{bson::doc, Client};
use serde::{Deserialize, Serialize};
use serial_test::serial;
use tokio;
use uuid::Uuid;
use crate::{db_provider::DBProvider, errors::DBError, mongodb_provider::MongoDBProvider};

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
async fn setup_db_provider(create_indexes: bool) -> Result<MongoDBProvider, DBError> {
    let db_provider = MongoDBProvider::new(
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
