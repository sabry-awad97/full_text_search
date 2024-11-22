use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, Clone, FromRow)]
pub struct SearchDocument {
    pub id: i32,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub enum DocumentEvent {
    Created(SearchDocument),
    Updated(SearchDocument),
    Deleted(i32),
}
