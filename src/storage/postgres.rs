use crate::models::SearchDocument;
use crate::storage::DocumentStorage;
use anyhow::Result;
use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};

pub struct PostgresStorage {
    pool: PgPool,
}

impl PostgresStorage {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        Self::init_database(&pool).await?;

        Ok(Self { pool })
    }

    async fn init_database(pool: &PgPool) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl DocumentStorage for PostgresStorage {
    async fn add_document(&self, title: &str, body: &str) -> Result<i32> {
        let doc = sqlx::query_as::<_, SearchDocument>(
            "INSERT INTO documents (title, body) VALUES ($1, $2) RETURNING id, title, body",
        )
        .bind(title)
        .bind(body)
        .fetch_one(&self.pool)
        .await?;

        Ok(doc.id)
    }

    async fn get_document(&self, id: i32) -> Result<SearchDocument> {
        let doc = sqlx::query_as::<_, SearchDocument>(
            "SELECT id, title, body FROM documents WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(doc)
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
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
        sqlx::query("DELETE FROM documents")
            .execute(&storage.pool)
            .await?;
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
        cleanup_test_db(&storage).await?;

        let id1 = storage.add_document("First Doc", "First Content").await?;
        let id2 = storage.add_document("Second Doc", "Second Content").await?;

        let doc1 = storage.get_document(id1).await?;
        let doc2 = storage.get_document(id2).await?;

        assert_eq!(doc1.title, "First Doc");
        assert_eq!(doc2.title, "Second Doc");

        cleanup_test_db(&storage).await?;
        Ok(())
    }
}
