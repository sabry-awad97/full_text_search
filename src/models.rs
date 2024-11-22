use crate::entities::documents::Model as SearchDocument;

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentEvent {
    Created(SearchDocument),
    Updated(SearchDocument),
    Deleted(i32),
}
