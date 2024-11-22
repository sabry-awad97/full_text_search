use crate::entities::documents::{self, Entity as Documents, Model as SearchDocument};
use crate::storage::DocumentStorage;
use anyhow::Result;
use async_trait::async_trait;
use sea_orm::ConnectionTrait;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;

pub struct PostgresStorage {
    db: Arc<DatabaseConnection>,
}

impl PostgresStorage {
    pub async fn new(database_url: &str) -> Result<Self> {
        let db = Database::connect(database_url).await?;
        Self::init_database(&db).await?;
        Ok(Self { db: Arc::new(db) })
    }

    async fn init_database(db: &DatabaseConnection) -> Result<()> {
        let schema = r#"
            CREATE TABLE IF NOT EXISTS documents (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT NOT NULL
            )
        "#;
        let stmt = sea_orm::Statement::from_string(db.get_database_backend(), schema.to_owned());
        db.execute(stmt).await?;
        Ok(())
    }
}

#[async_trait]
impl DocumentStorage for PostgresStorage {
    async fn add_document(&self, title: &str, body: &str) -> Result<i32> {
        let document = documents::ActiveModel {
            title: Set(title.to_owned()),
            body: Set(body.to_owned()),
            ..Default::default()
        };

        let res = document.insert(self.db.as_ref()).await?;
        Ok(res.id)
    }

    async fn get_document(&self, id: i32) -> Result<SearchDocument> {
        let doc = Documents::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Document not found: {}", id))?;
        Ok(SearchDocument {
            id: doc.id,
            title: doc.title,
            body: doc.body,
        })
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        Documents::delete_by_id(id).exec(self.db.as_ref()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotenvy::dotenv;

    async fn setup_test_db() -> Result<PostgresStorage> {
        dotenv()?;
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        PostgresStorage::new(&database_url).await
    }

    async fn cleanup_test_db(storage: &PostgresStorage) -> Result<()> {
        let stmt = sea_orm::Statement::from_string(
            storage.db.get_database_backend(),
            "TRUNCATE TABLE documents RESTART IDENTITY CASCADE".to_owned(),
        );
        storage.db.execute(stmt).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_crud_operations() -> Result<()> {
        let storage = setup_test_db().await?;
        cleanup_test_db(&storage).await?;

        // Test adding a document
        let id = storage.add_document("Test Title", "Test Content").await?;

        // Test retrieving the document
        let doc = storage.get_document(id).await?;
        assert_eq!(doc.title, "Test Title");
        assert_eq!(doc.body, "Test Content");

        // Test deleting the document
        storage.delete_document(id).await?;

        // Verify deletion
        let result = storage.get_document(id).await;
        assert!(result.is_err());

        cleanup_test_db(&storage).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_documents() -> Result<()> {
        let storage = setup_test_db().await?;
        println!("Test DB setup complete");
        cleanup_test_db(&storage).await?;
        println!("Test DB cleaned up");

        let id1 = storage.add_document("First Doc", "First Content").await?;
        println!("Added first document with id: {}", id1);
        let id2 = storage.add_document("Second Doc", "Second Content").await?;
        println!("Added second document with id: {}", id2);

        let doc1 = storage.get_document(id1).await?;
        println!("Retrieved first document: {} - {}", doc1.title, doc1.body);
        let doc2 = storage.get_document(id2).await?;
        println!("Retrieved second document: {} - {}", doc2.title, doc2.body);

        assert_eq!(doc1.title, "First Doc");
        assert_eq!(doc2.title, "Second Doc");

        cleanup_test_db(&storage).await?;
        println!("Final cleanup complete");
        Ok(())
    }
}
