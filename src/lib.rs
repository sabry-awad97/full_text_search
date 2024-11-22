pub mod entities;
mod models;
pub mod search;
pub mod storage;

pub use models::{DocumentEvent, SearchDocument};

use anyhow::Result;
use search::SearchIndex;
use storage::DocumentStorage;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

pub struct SearchEngine<S: DocumentStorage, I: SearchIndex> {
    storage: S,
    index: I,
    notification_tx: broadcast::Sender<DocumentEvent>,
}

impl<S: DocumentStorage, I: SearchIndex> SearchEngine<S, I> {
    pub fn new(storage: S, index: I) -> Self {
        let (notification_tx, _) = broadcast::channel(100);
        Self {
            storage,
            index,
            notification_tx,
        }
    }

    pub fn subscribe(&self) -> BroadcastStream<DocumentEvent> {
        BroadcastStream::new(self.notification_tx.subscribe())
    }

    pub async fn add_document(&self, title: &str, body: &str) -> Result<i32> {
        let id = self.storage.add_document(title, body).await?;
        let mut index = self.index.clone();
        index.add_document(title, body).await?;

        let doc = SearchDocument {
            id,
            title: title.to_string(),
            body: body.to_string(),
        };
        let _ = self.notification_tx.send(DocumentEvent::Created(doc));

        Ok(id)
    }

    pub async fn get_document(&self, id: i32) -> Result<SearchDocument> {
        self.storage.get_document(id).await
    }

    pub async fn delete_document(&self, id: i32) -> Result<()> {
        let mut index = self.index.clone();
        index.delete_document(id).await?;
        self.storage.delete_document(id).await?;
        let _ = self.notification_tx.send(DocumentEvent::Deleted(id));
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchDocument>> {
        let results = self.index.search(query).await?;
        let mut documents = Vec::new();

        for (id, _score) in results {
            if let Ok(doc) = self.storage.get_document(id).await {
                documents.push(doc);
            }
        }

        Ok(documents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::memory::MemoryIndex;
    use crate::storage::memory::MemoryStorage;
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_search_engine_operations() -> Result<()> {
        let storage = MemoryStorage::new();
        let index = MemoryIndex::new();
        let engine = SearchEngine::new(storage, index);

        // Add documents
        let id1 = engine
            .add_document("Rust Programming", "Learn Rust language")
            .await?;
        let id2 = engine
            .add_document("Python Guide", "Python programming tutorial")
            .await?;

        // Test document retrieval
        let doc1 = engine.get_document(id1).await?;
        assert_eq!(doc1.title, "Rust Programming");

        // Test search
        let results = engine.search("rust").await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id1);

        // Test delete
        engine.delete_document(id2).await?;
        let results = engine.search("python").await?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_notifications() -> Result<()> {
        let storage = MemoryStorage::new();
        let index = MemoryIndex::new();
        let engine = SearchEngine::new(storage, index);

        let mut notifications = engine.subscribe();

        // Add a document
        let id = engine.add_document("Test Doc", "Test content").await?;

        // Wait a bit for notification
        sleep(Duration::from_millis(100)).await;

        if let Some(Ok(event)) = notifications.next().await {
            match event {
                DocumentEvent::Created(doc) => {
                    assert_eq!(doc.id, id);
                    assert_eq!(doc.title, "Test Doc");
                }
                _ => panic!("Expected Created event"),
            }
        }

        // Delete the document
        engine.delete_document(id).await?;

        // Wait a bit for notification
        sleep(Duration::from_millis(100)).await;

        if let Some(Ok(event)) = notifications.next().await {
            match event {
                DocumentEvent::Deleted(deleted_id) => {
                    assert_eq!(deleted_id, id);
                }
                _ => panic!("Expected Deleted event"),
            }
        }

        Ok(())
    }
}
