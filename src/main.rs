use anyhow::Result;
use dotenvy::dotenv;
use full_text_search::search::OptimizedIndex;
use full_text_search::storage::postgres::PostgresStorage;
use full_text_search::{DocumentEvent, SearchEngine};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Get database URL from environment
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let path = PathBuf::from("./search_index");
    if !path.exists() {
        std::fs::create_dir_all(&path)?;
    }

    // Create storage and index
    let storage = PostgresStorage::new(&database_url).await?;
    let index = OptimizedIndex::new(path)?;

    // Create search engine
    let search_engine = Arc::new(SearchEngine::new(storage, index));
    let search_engine_clone = Arc::clone(&search_engine);

    // Subscribe to document updates
    let mut notification_stream = search_engine.subscribe();
    tokio::spawn(async move {
        while let Some(Ok(event)) = notification_stream.next().await {
            match event {
                DocumentEvent::Created(doc) => {
                    println!("New document created: {} (ID: {})", doc.title, doc.id);
                }
                DocumentEvent::Updated(doc) => {
                    println!("Document updated: {} (ID: {})", doc.title, doc.id);
                }
                DocumentEvent::Deleted(id) => {
                    println!("Document deleted: ID {}", id);
                }
            }
        }
    });

    // Add some test documents
    let doc1_id = search_engine
        .add_document("Rust Programming", "Rust is a systems programming language")
        .await?;
    println!("Added document with ID: {}", doc1_id);

    let doc2_id = search_engine
        .add_document(
            "Document Management",
            "Learn how to manage documents efficiently",
        )
        .await?;
    println!("Added document with ID: {}", doc2_id);

    // Search for documents
    let results = search_engine_clone.search("Rust").await?;
    println!("\nSearch results for 'Rust':");
    if results.is_empty() {
        println!("No documents found.");
    } else {
        for doc in results {
            println!("Title: {}", doc.title);
            println!("ID: {}", doc.id);
            println!("Content: {}", doc.body);
            println!("-------------------");
        }
    }

    Ok(())
}
