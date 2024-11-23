use crate::search::SearchIndex;
use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    doc,
    query::QueryParser,
    schema::{Field, Schema, SchemaBuilder, TEXT},
    Index, IndexWriter, Term,
};

/// Optimized search index implementation using Tantivy with memory-mapped files
/// and document ID mapping for efficient lookups
#[derive(Clone)]
pub struct OptimizedIndex {
    index: Arc<Index>,
    title_field: Field,
    body_field: Field,
    writer: Arc<Mutex<IndexWriter>>,
    // Map our document IDs to Tantivy's doc addresses
    id_mapping: Arc<DashMap<i32, u32>>,
    next_id: Arc<Mutex<i32>>,
}

impl OptimizedIndex {
    pub fn new(path: PathBuf) -> Result<Self> {
        let schema = Self::create_schema();
        let directory = MmapDirectory::open(&path)?;
        let index = Index::open_or_create(directory, schema.clone())?;

        // Configure larger heap size for better indexing performance
        let writer = index.writer_with_num_threads(2, 200_000_000)?;

        Ok(Self {
            index: Arc::new(index),
            title_field: schema.get_field("title").unwrap(),
            body_field: schema.get_field("body").unwrap(),
            writer: Arc::new(Mutex::new(writer)),
            id_mapping: Arc::new(DashMap::new()),
            next_id: Arc::new(Mutex::new(1)),
        })
    }

    fn create_schema() -> Schema {
        let mut builder = SchemaBuilder::new();

        // Configure fields for optimal performance
        let title_options = TEXT
            .set_indexing_options(
                tantivy::schema::TextFieldIndexing::default()
                    .set_tokenizer("en_stem")
                    .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        let body_options = TEXT
            .set_indexing_options(
                tantivy::schema::TextFieldIndexing::default()
                    .set_tokenizer("en_stem")
                    .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        builder.add_text_field("title", title_options);
        builder.add_text_field("body", body_options);
        builder.build()
    }

    fn get_next_id(&self) -> i32 {
        let mut id = self.next_id.lock().unwrap();
        let current = *id;
        *id += 1;
        current
    }
}

#[async_trait]
impl SearchIndex for OptimizedIndex {
    async fn add_document(&self, title: &str, body: &str) -> Result<()> {
        let doc_id = self.get_next_id();
        let doc = doc!(
            self.title_field => title,
            self.body_field => body,
        );

        let mut writer = self.writer.lock().unwrap();
        let doc_address = writer.add_document(doc)?;
        writer.commit()?;

        // Store the mapping between our ID and Tantivy's doc address
        self.id_mapping.insert(doc_id, doc_address as u32);

        Ok(())
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        if let Some((_, doc_address)) = self.id_mapping.remove(&id) {
            let mut writer = self.writer.lock().unwrap();
            // Create a term that uniquely identifies the document
            let term = Term::from_field_u64(self.title_field, doc_address as u64);
            writer.delete_term(term);
            writer.commit()?;
        }
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();

        // Create a query parser that searches both title and body
        let query_parser =
            QueryParser::for_index(&self.index, vec![self.title_field, self.body_field]);

        // Parse and execute the query
        let query = query_parser.parse_query(query)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

        // Convert results back to our document IDs with scores
        let results: Vec<(i32, f32)> = top_docs
            .iter()
            .filter_map(|(score, doc_address)| {
                // Find our document ID from the Tantivy doc address
                self.id_mapping
                    .iter()
                    .find(|entry| *entry.value() == doc_address.doc_id)
                    .map(|entry| (*entry.key(), *score))
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_basic_operations() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        // Test document addition
        index
            .add_document("Rust Programming", "Learn Rust language")
            .await?;
        index
            .add_document("Python Guide", "Python programming tutorial")
            .await?;

        // Test basic search
        let results = index.search("rust").await?;
        assert_eq!(results.len(), 1);

        let results = index.search("programming").await?;
        assert_eq!(results.len(), 2);

        // Test document deletion
        index.delete_document(1).await?;
        let results = index.search("rust").await?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_relevance() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        // Add documents with varying relevance
        index
            .add_document(
                "Rust Programming Guide",
                "Comprehensive guide to Rust programming language",
            )
            .await?;
        index
            .add_document(
                "Programming Languages",
                "Brief mention of Rust among other languages",
            )
            .await?;

        // Test search relevance
        let results = index.search("rust programming").await?;
        assert_eq!(results.len(), 2);

        // First result should have higher score due to more relevant content
        assert!(results[0].1 > results[1].1);

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_operations() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;
        let index_clone = index.clone();

        // Spawn concurrent add operations
        let add_task = tokio::spawn(async move {
            for i in 0..5 {
                index_clone
                    .add_document(
                        &format!("Document {}", i),
                        &format!("Content for document {}", i),
                    )
                    .await?;
            }
            Ok::<(), anyhow::Error>(())
        });

        // Perform searches while adding
        for i in 0..3 {
            let results = index.search(&format!("document {}", i)).await?;
            // Number of results may vary depending on timing
            assert!(results.len() <= (i + 1));
        }

        add_task.await??;

        // Verify final state
        let results = index.search("document").await?;
        assert_eq!(results.len(), 5);

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_and_edge_cases() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        // Test empty search
        let results = index.search("").await?;
        assert_eq!(results.len(), 0);

        // Test search with no documents
        let results = index.search("test").await?;
        assert_eq!(results.len(), 0);

        // Test delete non-existent document
        index.delete_document(999).await?;

        // Test adding and searching documents with special characters
        index
            .add_document(
                "Special !@#$%^&*()",
                "Content with special chars: !@#$%^&*()",
            )
            .await?;
        let results = index.search("special").await?;
        assert_eq!(results.len(), 1);

        Ok(())
    }
}
