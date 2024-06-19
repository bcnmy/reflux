use crate::db_provider::DBProvider;
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Account {
    pub chain_id: String,
    pub is_enabled: bool,
    pub account_address: String,
    pub account_type: String,
}

#[derive(Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub accounts: Vec<Account>,
}

impl User {
    pub async fn save<T: DBProvider>(
        &self,
        db_provider: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        db_provider.create(self).await
    }

    pub async fn find<T: DBProvider>(
        db_provider: &T,
        user_id: &str,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let query = doc! { "user_id": user_id };
        db_provider.read(&query).await
    }

    pub async fn update<T: DBProvider>(
        &self,
        db_provider: &T,
        update: &Self,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = doc! { "user_id": &self.user_id };
        db_provider.update(&query, update).await
    }

    pub async fn delete<T: DBProvider>(
        &self,
        db_provider: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = doc! { "user_id": &self.user_id };
        db_provider.delete(&query).await
    }
}
