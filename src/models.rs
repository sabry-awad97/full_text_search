use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SearchDocument {
    pub id: i32,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentEvent {
    Created(SearchDocument),
    Updated(SearchDocument),
    Deleted(i32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_document_equality() {
        let doc1 = SearchDocument {
            id: 1,
            title: "Test".to_string(),
            body: "Test body".to_string(),
        };
        let doc2 = SearchDocument {
            id: 1,
            title: "Test".to_string(),
            body: "Test body".to_string(),
        };
        let doc3 = SearchDocument {
            id: 2,
            title: "Test".to_string(),
            body: "Test body".to_string(),
        };

        assert_eq!(doc1, doc2);
        assert_ne!(doc1, doc3);
    }

    #[test]
    fn test_document_event_equality() {
        let doc = SearchDocument {
            id: 1,
            title: "Test".to_string(),
            body: "Test body".to_string(),
        };

        let event1 = DocumentEvent::Created(doc.clone());
        let event2 = DocumentEvent::Created(doc.clone());
        let event3 = DocumentEvent::Updated(doc.clone());
        let event4 = DocumentEvent::Deleted(1);
        let event5 = DocumentEvent::Deleted(1);

        assert_eq!(event1, event2);
        assert_ne!(event1, event3);
        assert_eq!(event4, event5);
    }
}
