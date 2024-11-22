use crate::entities::documents::Model as SearchDocument;
use crate::storage::DocumentStorage;
use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;

#[derive(Clone, Default)]
pub struct MemoryStorage {
    documents: DashMap<i32, SearchDocument>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DocumentStorage for MemoryStorage {
    async fn add_document(&self, title: &str, body: &str) -> Result<i32> {
        let id = self.documents.len() as i32 + 1;
        let doc = SearchDocument {
            id,
            title: title.to_string(),
            body: body.to_string(),
        };
        self.documents.insert(id, doc);
        Ok(id)
    }

    async fn get_document(&self, id: i32) -> Result<SearchDocument> {
        self.documents
            .get(&id)
            .map(|doc| doc.clone())
            .ok_or_else(|| anyhow::anyhow!("Document not found"))
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        self.documents.remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_document() -> Result<()> {
        let storage = MemoryStorage::new();
        let id = storage.add_document("Test Title", "Test Content").await?;
        let doc = storage.get_document(id).await?;
        assert_eq!(doc.title, "Test Title");
        assert_eq!(doc.body, "Test Content");
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_document() -> Result<()> {
        let storage = MemoryStorage::new();
        let id = storage.add_document("Test Title", "Test Content").await?;
        storage.delete_document(id).await?;
        let result = storage.get_document(id).await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_get_nonexistent_document() -> Result<()> {
        let storage = MemoryStorage::new();
        let result = storage.get_document(1).await;
        assert!(result.is_err());
        Ok(())
    }
}
