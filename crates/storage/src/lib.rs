use mongodb::{bson::doc, Client, Collection, bson};
use std::error::Error;

pub struct StorageProvider {
    pub client: Client,
    db_name: String,
    collection_name: String,
}

impl StorageProvider {
    pub async fn new(client: Client, db_name: String, collection_name: String) -> Result<Self, Box<dyn Error>> {
        Ok(Self { client, db_name, collection_name })
    }

    pub fn get_collection<T>(&self) -> Collection<T> {
        self.client.database(&self.db_name).collection(&self.collection_name)
    }

    pub async fn get_user_accounts(&self, address: &String) -> Option<Vec<String>> {
        let client = self.get_collection();

        client.find_one(Some(doc! { "address": address }), None)
            .await
            .ok()
            .flatten()
            .map(|doc: bson::Document| {
                doc.get_array("user_ids").unwrap().iter().map(|x| x.to_string()).collect()
            })
    }

    pub async fn get_user_id(&self, address: &String) -> Option<String> {
        let client = self.get_collection();

        client.find_one(Some(doc! { "address": address }), None)
            .await
            .ok()
            .flatten()
            .map(|doc: bson::Document| {
                doc.get_str("user_id").unwrap().to_string()
            })
    }
}
