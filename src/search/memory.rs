use crate::search::SearchIndex;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct MemoryIndex {
    documents: Arc<Mutex<HashMap<i32, (String, String)>>>,
}

impl MemoryIndex {
    pub fn new() -> Self {
        Self {
            documents: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl SearchIndex for MemoryIndex {
    async fn add_document(&mut self, title: &str, body: &str) -> Result<()> {
        let mut documents = self.documents.lock().unwrap();
        let id = documents.len() as i32 + 1;
        documents.insert(id, (title.to_string(), body.to_string()));
        Ok(())
    }

    async fn delete_document(&mut self, id: i32) -> Result<()> {
        let mut documents = self.documents.lock().unwrap();
        documents.remove(&id);
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>> {
        let documents = self.documents.lock().unwrap();
        let query = query.to_lowercase();
        let mut results = Vec::new();

        for (&id, (title, body)) in documents.iter() {
            if title.to_lowercase().contains(&query) || body.to_lowercase().contains(&query) {
                results.push((id, 1.0));
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_index_and_search() -> Result<()> {
        let mut index = MemoryIndex::new();

        // Add test documents
        index
            .add_document("Rust Programming", "Learn Rust language")
            .await?;
        index
            .add_document("Python Guide", "Python programming tutorial")
            .await?;

        // Test search
        let results = index.search("rust").await?;
        assert_eq!(results.len(), 1);

        let results = index.search("programming").await?;
        assert_eq!(results.len(), 2);

        let results = index.search("javascript").await?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_document() -> Result<()> {
        let mut index = MemoryIndex::new();

        // Add a document
        index.add_document("Test Doc", "Test content").await?;

        // Search to verify document was added
        let results = index.search("test").await?;
        assert_eq!(results.len(), 1);

        // Delete the document
        index.delete_document(1).await?;

        // Search to verify document was deleted
        let results = index.search("test").await?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_case_insensitive_search() -> Result<()> {
        let mut index = MemoryIndex::new();

        index
            .add_document("UPPERCASE TITLE", "UPPERCASE CONTENT")
            .await?;
        index
            .add_document("lowercase title", "lowercase content")
            .await?;

        let results = index.search("UPPERCASE").await?;
        assert_eq!(results.len(), 1);

        let results = index.search("uppercase").await?;
        assert_eq!(results.len(), 1);

        let results = index.search("LOWERCASE").await?;
        assert_eq!(results.len(), 1);

        let results = index.search("lowercase").await?;
        assert_eq!(results.len(), 1);

        Ok(())
    }
}
