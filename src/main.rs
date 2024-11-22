use anyhow::Result;
use dotenvy::dotenv;
use full_text_search::search::tantivy_index::TantivyIndex;
use full_text_search::storage::postgres::PostgresStorage;
use full_text_search::{DocumentEvent, SearchEngine};
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Get database URL from environment
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // Create storage and index
    let storage = PostgresStorage::new(&database_url).await?;
    let index = TantivyIndex::new()?;

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
            "This is a document about managing documents",
        )
        .await?;
    println!("Added document with ID: {}", doc2_id);

    // Search for documents
    let results = search_engine_clone.search("rust").await?;
    for doc in results {
        println!("Found document: {} (ID: {})", doc.title, doc.id);
    }

    let results = search_engine_clone.search("document").await?;
    for doc in results {
        println!("Found document: {} (ID: {})", doc.title, doc.id);
    }

    println!("Press Ctrl+C to exit");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
