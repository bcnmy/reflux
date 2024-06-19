use mongodb::bson;
use thiserror::Error;

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
