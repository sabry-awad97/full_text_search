use crate::SearchDocument;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DocumentStorage: Send + Sync {
    async fn add_document(&self, title: &str, body: &str) -> Result<i32>;
    async fn get_document(&self, id: i32) -> Result<SearchDocument>;
    async fn delete_document(&self, id: i32) -> Result<()>;
}

pub mod memory;
pub mod postgres;
