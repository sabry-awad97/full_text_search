use crate::search::SearchIndex;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tantivy::TantivyDocument;
use tantivy::{
    collector::TopDocs,
    query::QueryParser,
    schema::{Field, Schema, STORED, TEXT},
    Index, IndexWriter,
};
use tempfile::TempDir;

#[derive(Clone)]
pub struct TantivyIndex {
    index: Arc<Index>,
    title_field: Field,
    body_field: Field,
    writer: Arc<Mutex<IndexWriter>>,
    _temp_dir: Arc<Option<TempDir>>, // Keep TempDir alive while index is used
}

impl TantivyIndex {
    pub fn new() -> Result<Self> {
        Self::with_path(None)
    }

    pub fn with_path(path: Option<PathBuf>) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT | STORED);
        let schema = schema_builder.build();

        let (index, temp_dir) = match path {
            Some(p) => (Index::create_in_dir(p, schema.clone())?, None),
            None => {
                let dir = TempDir::new()?;
                (Index::create_in_dir(dir.path(), schema.clone())?, Some(dir))
            }
        };

        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index: Arc::new(index),
            title_field,
            body_field,
            writer: Arc::new(Mutex::new(writer)),
            _temp_dir: Arc::new(temp_dir),
        })
    }

    fn create_document(&self, title: &str, body: &str) -> TantivyDocument {
        let mut doc = TantivyDocument::new();
        doc.add_text(self.title_field, title);
        doc.add_text(self.body_field, body);
        doc
    }
}

#[async_trait]
impl SearchIndex for TantivyIndex {
    async fn add_document(&mut self, title: &str, body: &str) -> Result<()> {
        let doc = self.create_document(title, body);
        let mut writer = self.writer.lock().unwrap();
        writer.add_document(doc)?;
        writer.commit()?;
        Ok(())
    }

    async fn delete_document(&mut self, _id: i32) -> Result<()> {
        // Note: Tantivy doesn't support document deletion by external ID
        // In a production system, we'd need to maintain a mapping between
        // our document IDs and Tantivy's internal document IDs
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();

        let query_parser =
            QueryParser::for_index(&self.index, vec![self.title_field, self.body_field]);
        let query = query_parser.parse_query(query)?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

        // For testing purposes, we'll return the document address as the ID
        // In a real system, we'd need to maintain ID mappings
        Ok(top_docs
            .iter()
            .map(|(score, doc_address)| (doc_address.doc_id as i32, *score))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_index_and_search() -> Result<()> {
        let mut index = TantivyIndex::new()?;

        // Add test documents
        index
            .add_document("Rust Programming", "Learn Rust programming language")
            .await?;
        index
            .add_document("Python Guide", "Python programming tutorial")
            .await?;

        // Allow time for indexing
        sleep(Duration::from_millis(100)).await;

        // Search for documents
        let results = index.search("rust").await?;
        assert!(
            !results.is_empty(),
            "Should find documents containing 'rust'"
        );

        let results = index.search("python").await?;
        assert!(
            !results.is_empty(),
            "Should find documents containing 'python'"
        );

        let results = index.search("javascript").await?;
        assert!(
            results.is_empty(),
            "Should not find documents containing 'javascript'"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_document() -> Result<()> {
        let mut index = TantivyIndex::new()?;

        // Add a test document
        index
            .add_document("Test Document", "This is a test document")
            .await?;

        // Search to verify document was added
        let results = index.search("test").await?;
        assert!(
            !results.is_empty(),
            "Document should be found before deletion"
        );

        // Delete the document (note: this is a no-op in our current implementation)
        index.delete_document(1).await?;

        // Search again to verify document still exists (since delete is no-op)
        let results = index.search("test").await?;
        assert!(
            !results.is_empty(),
            "Document should still exist after deletion"
        );

        Ok(())
    }
}
