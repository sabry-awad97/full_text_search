use anyhow::Result;
use dotenvy::dotenv;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::postgres::{PgListener, PgPool, PgPoolOptions};
use sqlx::{FromRow, Row};
use std::fs;
use std::sync::Arc;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

const CHANNEL_NAME: &str = "document_updates";

#[derive(Debug, Serialize, Deserialize, Clone, FromRow)]
struct SearchDocument {
    id: i32,
    title: String,
    body: String,
}

#[derive(Debug, Clone)]
enum DocumentEvent {
    Created(SearchDocument),
    Updated(SearchDocument),
    Deleted(i32),
}

struct SearchEngine {
    pool: PgPool,
    index: Index,
    schema: Schema,
    title_field: Field,
    body_field: Field,
    id_field: Field,
    notification_tx: broadcast::Sender<DocumentEvent>,
}

impl SearchEngine {
    async fn new() -> Result<Self> {
        // Load environment variables from .env file
        dotenv()?;

        // Get database URL from environment
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

        // Extract the base URL (without database name) for initial connection
        let base_url = database_url.rsplit_once('/').map(|(base, _)| base).unwrap();
        let postgres_url = format!("{}/postgres", base_url);

        // Connect to the default database first
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&postgres_url)
            .await?;

        // Create our database (this might fail if it already exists, which is fine)
        let _ = sqlx::query("CREATE DATABASE full_text_search")
            .execute(&pool)
            .await;

        // Now connect to our database
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;

        // Create the documents table if it doesn't exist
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        // Drop existing trigger and function
        sqlx::query(
            r#"
            DROP TRIGGER IF EXISTS notify_document_changes ON documents
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            DROP FUNCTION IF EXISTS notify_document_changes()
            "#,
        )
        .execute(&pool)
        .await?;

        // Create notification function
        sqlx::query(
            r#"
            CREATE OR REPLACE FUNCTION notify_document_changes()
            RETURNS trigger AS $$
            DECLARE
                payload JSON;
            BEGIN
                IF (TG_OP = 'DELETE') THEN
                    payload = json_build_object(
                        'operation', TG_OP,
                        'record', row_to_json(OLD)
                    );
                ELSE
                    payload = json_build_object(
                        'operation', TG_OP,
                        'record', row_to_json(NEW)
                    );
                END IF;

                PERFORM pg_notify('document_updates', payload::text);
                RETURN NULL;
            END;
            $$ LANGUAGE plpgsql
            "#,
        )
        .execute(&pool)
        .await?;

        // Create trigger
        sqlx::query(
            r#"
            CREATE TRIGGER notify_document_changes
            AFTER INSERT OR UPDATE OR DELETE ON documents
            FOR EACH ROW EXECUTE FUNCTION notify_document_changes()
            "#,
        )
        .execute(&pool)
        .await?;

        // Create the schema
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_i64_field("id", STORED);
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT | STORED);
        let schema = schema_builder.build();

        // Create index directory if it doesn't exist
        fs::create_dir_all("index")?;

        // Open existing index or create new one
        let dir = tantivy::directory::MmapDirectory::open("index")?;
        let index = Index::open_or_create(dir, schema.clone())?;

        // Create broadcast channel for notifications
        let (notification_tx, _) = broadcast::channel(100);

        let engine = SearchEngine {
            pool,
            index,
            schema,
            title_field,
            body_field,
            id_field,
            notification_tx,
        };

        // Start the notification listener
        engine.start_notification_listener().await?;

        Ok(engine)
    }

    async fn start_notification_listener(&self) -> Result<()> {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen(CHANNEL_NAME).await?;

        let notification_tx = self.notification_tx.clone();
        tokio::spawn(async move {
            let mut stream = listener.into_stream();
            while let Some(notification) = stream.next().await {
                let payload: Value = serde_json::from_str(
                    notification.expect("Failed to get notification").payload(),
                )
                .expect("Failed to parse notification payload");

                let operation = payload["operation"].as_str().unwrap_or("");
                let record = &payload["record"];

                if let Ok(doc) = serde_json::from_value::<SearchDocument>(record.clone()) {
                    let event = match operation {
                        "INSERT" => Some(DocumentEvent::Created(doc)),
                        "UPDATE" => Some(DocumentEvent::Updated(doc)),
                        "DELETE" => Some(DocumentEvent::Deleted(doc.id)),
                        _ => None,
                    };

                    if let Some(event) = event {
                        notification_tx
                            .send(event)
                            .expect("Failed to send notification");
                    }
                }
            }
        });

        Ok(())
    }

    fn subscribe(&self) -> BroadcastStream<DocumentEvent> {
        BroadcastStream::new(self.notification_tx.subscribe())
    }

    async fn add_document(&self, title: &str, body: &str) -> Result<i32> {
        // Store in PostgreSQL
        let doc = sqlx::query(
            "INSERT INTO documents (title, body) VALUES ($1, $2) RETURNING id, title, body",
        )
        .bind(title)
        .bind(body)
        .fetch_one(&self.pool)
        .await?;

        // Index in Tantivy
        let mut index_writer = self.index.writer(50_000_000)?;
        let doc_id = doc.get::<i32, _>("id");
        index_writer.add_document(doc!(
            self.title_field => title,
            self.body_field => body,
            self.id_field => doc_id as i64,
        ))?;
        index_writer.commit()?;

        Ok(doc_id)
    }

    async fn search(&self, query_str: &str) -> Result<Vec<SearchDocument>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let query_parser =
            QueryParser::for_index(&self.index, vec![self.title_field, self.body_field]);

        let query = query_parser.parse_query(query_str)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc = searcher.doc(doc_address)?;
            let id = retrieved_doc
                .get_first(self.id_field)
                .unwrap()
                .as_i64()
                .unwrap() as i32;

            if let Ok(doc) = sqlx::query_as::<_, SearchDocument>(
                "SELECT id, title, body FROM documents WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            {
                results.push(doc);
            }
        }

        Ok(results)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let search_engine = Arc::new(SearchEngine::new().await?);
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
