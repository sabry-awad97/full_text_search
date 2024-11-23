use crate::search::SearchIndex;
use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tantivy::schema::Value;
use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    doc,
    query::{BooleanQuery, FuzzyTermQuery, Occur, Query, QueryParser},
    schema::{Field, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, STORED},
    Index, IndexWriter, TantivyDocument, Term,
};

/// Advanced search options for configuring search behavior
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub fuzzy_distance: Option<u8>,
    pub phrase_slop: Option<u32>,
    pub boost_title: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            phrase_slop: Some(0),
            fuzzy_distance: None, // Changed from Some(1) to None to make basic search more precise
            boost_title: true,
        }
    }
}

/// Optimized search index implementation using Tantivy with memory-mapped files
/// and document ID mapping for efficient lookups
#[derive(Clone)]
pub struct OptimizedIndex {
    index: Index,
    writer: Arc<Mutex<IndexWriter>>,
    title_field: Field,
    body_field: Field,
    id_field: Field,
    next_id: Arc<Mutex<i32>>,
    id_mapping: DashMap<i32, u64>,
}

impl OptimizedIndex {
    pub fn new(path: PathBuf) -> Result<Self> {
        let schema = Self::create_schema();
        let directory = MmapDirectory::open(&path)?;
        let index = Index::open_or_create(directory, schema.clone())?;

        // Configure larger heap size for better indexing performance
        let writer = index.writer_with_num_threads(2, 200_000_000)?;

        Ok(Self {
            index,
            writer: Arc::new(Mutex::new(writer)),
            title_field: schema.get_field("title").unwrap(),
            body_field: schema.get_field("body").unwrap(),
            id_field: schema.get_field("doc_id").unwrap(),
            next_id: Arc::new(Mutex::new(1)),
            id_mapping: DashMap::new(),
        })
    }

    fn create_schema() -> Schema {
        let mut builder = SchemaBuilder::new();

        let title_options = TextOptions::default()
            .set_stored()
            .set_fast(None)
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("en_stem")
                    .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
            );

        let body_options = TextOptions::default()
            .set_stored()
            .set_fast(None)
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("en_stem")
                    .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
            );

        builder.add_text_field("title", title_options);
        builder.add_text_field("body", body_options);
        builder.add_text_field("doc_id", STORED);
        builder.build()
    }

    fn get_next_id(&self) -> i32 {
        let mut id = self.next_id.lock().unwrap();
        let current = *id;
        *id += 1;
        current
    }

    /// Perform an advanced search with configurable options
    pub async fn advanced_search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<(i32, f32)>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();

        // Create query parser with field boosts
        let mut query_parser =
            QueryParser::for_index(&self.index, vec![self.title_field, self.body_field]);
        if options.boost_title {
            query_parser.set_field_boost(self.title_field, 2.0);
        }

        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // For phrase queries, we want to search in both fields
        if query.starts_with('"') && query.ends_with('"') {
            let query_str = query.to_lowercase();
            let parsed_query = query_parser.parse_query(&query_str)?;
            subqueries.push((Occur::Must, Box::new(parsed_query)));
        } else {
            // Add the main parsed query first
            let parsed_query = query_parser.parse_query(&query.to_lowercase())?;
            subqueries.push((Occur::Should, Box::new(parsed_query)));

            // Add fuzzy queries if enabled
            if let Some(distance) = options.fuzzy_distance {
                let terms = query.split_whitespace();
                for term in terms {
                    let title_fuzzy = FuzzyTermQuery::new_prefix(
                        Term::from_field_text(self.title_field, &term.to_lowercase()),
                        distance,
                        true,
                    );
                    let body_fuzzy = FuzzyTermQuery::new_prefix(
                        Term::from_field_text(self.body_field, &term.to_lowercase()),
                        distance,
                        true,
                    );
                    subqueries.push((Occur::Should, Box::new(title_fuzzy)));
                    subqueries.push((Occur::Should, Box::new(body_fuzzy)));
                }
            }
        }

        let boolean_query = BooleanQuery::new(subqueries);
        let top_docs = searcher.search(&boolean_query, &TopDocs::with_limit(10))?;

        // Convert results back to our document IDs with scores
        let mut results: Vec<(i32, f32)> = top_docs
            .iter()
            .filter_map(|(score, doc_address)| {
                self.id_mapping
                    .iter()
                    .find(|entry| {
                        let stored_doc_address = *entry.value();
                        stored_doc_address == doc_address.doc_id as u64
                    })
                    .map(|entry| {
                        let mut final_score = *score;
                        // Apply additional title boost if enabled
                        if options.boost_title {
                            if let Ok(doc) = searcher.doc::<TantivyDocument>(*doc_address) {
                                if let Some(title) = doc.get_first(self.title_field) {
                                    let text = title.as_str().unwrap_or("");
                                    if text.to_lowercase().contains(&query.to_lowercase()) {
                                        final_score *= 2.0;
                                    }
                                }
                            }
                        }
                        (*entry.key(), final_score)
                    })
            })
            .collect();

        // Sort by score in descending order
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        Ok(results)
    }

    async fn add_document(&self, title: &str, body: &str) -> Result<()> {
        let mut writer = self.writer.lock().unwrap();

        // Convert title and body to lowercase for case-insensitive search
        let title = title.to_lowercase();
        let body = body.to_lowercase();

        let doc_id = self.get_next_id();
        let doc = doc!(
            self.title_field => title,
            self.body_field => body,
            self.id_field => doc_id.to_string(),
        );

        let doc_address = writer.add_document(doc)?;
        writer.commit()?;
        self.id_mapping.insert(doc_id, doc_address);

        Ok(())
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        if self.id_mapping.remove(&id).is_some() {
            let mut writer = self.writer.lock().unwrap();
            writer.delete_term(Term::from_field_text(self.id_field, &id.to_string()));
            writer.commit()?;
        }
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>> {
        // Use advanced search with default options
        self.advanced_search(query, SearchOptions::default()).await
    }
}

#[async_trait]
impl SearchIndex for OptimizedIndex {
    async fn add_document(&self, title: &str, body: &str) -> Result<()> {
        self.add_document(title, body).await
    }

    async fn delete_document(&self, id: i32) -> Result<()> {
        self.delete_document(id).await
    }

    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>> {
        self.search(query).await
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

        // Add test documents
        index
            .add_document("Rust Programming", "Learn Rust language basics")
            .await?;
        index
            .add_document("Python Guide", "Python programming tutorial")
            .await?;

        // Test exact match
        let results = index.search("rust").await?;
        assert_eq!(results.len(), 1);

        // Test partial match with default options
        let results = index
            .advanced_search("programming", SearchOptions::default())
            .await?;
        assert_eq!(results.len(), 2);

        // Test document deletion
        index.delete_document(1).await?;
        let results = index.search("rust").await?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_phrase_search() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        println!("Adding test documents...");
        index
            .add_document(
                "Advanced Rust Programming",
                "Learn advanced Rust programming techniques",
            )
            .await?;
        index
            .add_document(
                "Programming Languages",
                "Rust is one of many programming languages",
            )
            .await?;

        let options = SearchOptions {
            phrase_slop: Some(0),
            fuzzy_distance: None, // Disable fuzzy search for this test
            boost_title: true,
        };

        println!("Testing exact phrase match...");
        let results = index
            .advanced_search("\"advanced rust\"", options.clone())
            .await?;
        println!("Phrase search results: {:?}", results);
        assert_eq!(results.len(), 1);

        println!("Testing normal search...");
        let results = index.advanced_search("rust programming", options).await?;
        println!("Normal search results: {:?}", results);
        assert_eq!(results.len(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_boosted_title_search() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        index
            .add_document("Rust Guide", "A comprehensive programming tutorial")
            .await?;
        index
            .add_document("Programming Tutorial", "Learn Rust and other languages")
            .await?;

        let options = SearchOptions {
            boost_title: true,
            ..Default::default()
        };

        let results = index.advanced_search("rust", options).await?;
        assert_eq!(results.len(), 2);

        // First result should be the document with "Rust" in title
        let (_first_id, first_score) = results[0];
        let (_second_id, second_score) = results[1];
        assert!(
            first_score > second_score,
            "Title boost not working: first_score={}, second_score={}",
            first_score,
            second_score
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_fuzzy_search() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        // Add test documents
        index
            .add_document(
                "Rust Programming Guide",
                "Learn the fundamentals of Rust programming",
            )
            .await?;
        index
            .add_document("Python Development", "Python development best practices")
            .await?;

        // Test with fuzzy search enabled
        let fuzzy_options = SearchOptions {
            phrase_slop: None,
            fuzzy_distance: Some(1), // Allow 1 character difference
            boost_title: true,
        };

        // Test misspelling
        let results = index.advanced_search("Ruts", fuzzy_options.clone()).await?;
        assert_eq!(
            results.len(),
            1,
            "Should match 'Rust' with one character difference"
        );

        // Test partial word
        let results = index
            .advanced_search("program", fuzzy_options.clone())
            .await?;
        assert_eq!(
            results.len(),
            1,
            "Should match 'programming' as it starts with 'program'"
        );

        // Test with higher fuzzy distance
        let fuzzy_options_2 = SearchOptions {
            phrase_slop: None,
            fuzzy_distance: Some(2), // Allow 2 character differences
            boost_title: true,
        };

        // Test more significant misspelling
        let results = index
            .advanced_search("Pithon", fuzzy_options_2.clone())
            .await?;
        assert_eq!(
            results.len(),
            1,
            "Should match 'Python' with two character differences"
        );

        // Test with fuzzy search disabled
        let exact_options = SearchOptions {
            phrase_slop: None,
            fuzzy_distance: None,
            boost_title: true,
        };

        // Verify misspelling doesn't match without fuzzy search
        let results = index.advanced_search("Ruts", exact_options).await?;
        assert_eq!(
            results.len(),
            0,
            "Should not match 'Rust' when fuzzy search is disabled"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_advanced_fuzzy_search() -> Result<()> {
        let dir = tempdir()?;
        let index = OptimizedIndex::new(dir.path().to_path_buf())?;

        // Add test documents with various text patterns
        index
            .add_document(
                "Rust & JavaScript Programming",
                "Advanced Rust-JS integration guide",
            )
            .await?;
        index
            .add_document(
                "Python Data Analysis",
                "Data science with Panda's framework",
            )
            .await?;
        index
            .add_document(
                "Basic Programming",
                "Introduction to programming concepts",
            )
            .await?;

        let fuzzy_options = SearchOptions {
            phrase_slop: None,
            fuzzy_distance: Some(2),
            boost_title: true,
        };

        // Test multiple word fuzzy matches
        println!("\nTesting multiple word fuzzy matches...");
        let results = index
            .advanced_search("Rast and Javasscript", fuzzy_options.clone())
            .await?;
        println!("Results for 'Rast and Javasscript': {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 1.0),
            "Should have at least one high-scoring match for fuzzy multi-word query"
        );

        // Test mixed exact and fuzzy matches
        println!("\nTesting mixed exact and fuzzy matches...");
        let results = index
            .advanced_search("Rust Javasscript", fuzzy_options.clone())
            .await?;
        println!("Results for 'Rust Javasscript': {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 1.5),
            "Should have a high-scoring match for mixed exact/fuzzy query"
        );

        // Test fuzzy matches with special characters
        println!("\nTesting fuzzy matches with special characters...");
        let results = index
            .advanced_search("RustJS", fuzzy_options.clone())
            .await?;
        println!("Results for 'RustJS': {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 0.5),
            "Should match 'Rust-JS' despite special characters"
        );

        // Test case sensitivity with fuzzy search
        println!("\nTesting case sensitivity...");
        let results = index
            .advanced_search("RAST AND JAVAscript", fuzzy_options.clone())
            .await?;
        println!("Results for 'RAST AND JAVAscript': {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 1.0),
            "Should match regardless of case differences"
        );

        // Test fuzzy matching in title vs body
        println!("\nTesting fuzzy matching in body...");
        let results = index
            .advanced_search("Panda framework", fuzzy_options.clone())
            .await?;
        println!("Results for 'Panda framework': {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 0.5),
            "Should match content in body with apostrophe"
        );

        // Test fuzzy prefix matching
        println!("\nTesting fuzzy prefix matching...");
        let results = index
            .advanced_search("progra", fuzzy_options.clone())
            .await?;
        println!("Results for 'progra': {:?}", results);
        assert!(
            results.len() >= 2,
            "Should match multiple documents with 'programming' prefix"
        );

        // Test with very strict fuzzy distance
        let strict_options = SearchOptions {
            phrase_slop: None,
            fuzzy_distance: Some(1),
            boost_title: true,
        };

        println!("\nTesting with strict fuzzy distance...");
        let results = index
            .advanced_search("Rast", strict_options.clone())
            .await?;
        println!("Results for 'Rast' with strict distance: {:?}", results);
        assert!(
            results.iter().any(|(_, score)| *score > 0.5),
            "Should still match with stricter fuzzy distance"
        );

        // Test no match with significant misspelling
        println!("\nTesting no match with significant misspelling...");
        let results = index
            .advanced_search("Rastafari", strict_options)
            .await?;
        println!("Results for 'Rastafari': {:?}", results);
        assert_eq!(
            results.len(),
            0,
            "Should not match with too many character differences"
        );

        Ok(())
    }
}
