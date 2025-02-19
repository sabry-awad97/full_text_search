use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait SearchIndex: Clone + Send + Sync + 'static {
    async fn add_document(&self, title: &str, body: &str) -> Result<()>;
    async fn delete_document(&self, id: i32) -> Result<()>;
    async fn search(&self, query: &str) -> Result<Vec<(i32, f32)>>;
}

pub mod memory;
pub mod tantivy_index;
pub mod optimized_index;

pub use optimized_index::OptimizedIndex;
