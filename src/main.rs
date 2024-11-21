use anyhow::Result;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index};

fn main() -> Result<()> {
    // Define the schema for our documents
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    let schema = schema_builder.build();

    // Create a temporary directory for the index
    let index_path = tempfile::tempdir()?;
    let index = Index::create_in_dir(&index_path, schema.clone())?;

    // Get field handles
    let title = schema.get_field("title").unwrap();
    let body = schema.get_field("body").unwrap();

    // Create a writer to add documents
    let mut index_writer = index.writer(50_000_000)?;

    // Add some sample documents
    index_writer.add_document(doc!(
        title => "The First Document",
        body => "This is the first document we've added to the search engine."
    ))?;

    index_writer.add_document(doc!(
        title => "The Second Document",
        body => "The second document is better than the first one."
    ))?;

    index_writer.add_document(doc!(
        title => "Introduction to Rust",
        body => "Rust is a systems programming language that runs blazingly fast."
    ))?;

    // Commit changes
    index_writer.commit()?;

    // Create a searcher
    let reader = index.reader()?;
    let searcher = reader.searcher();

    // Create a query parser
    let query_parser = QueryParser::for_index(&index, vec![title, body]);

    // Search for documents containing "rust"
    let query = query_parser.parse_query("rust")?;
    let top_docs = searcher.search(&query, &TopDocs::with_limit(2))?;

    println!("\nSearch results for 'rust':");
    for (_score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
        println!(
            "Title: {}",
            retrieved_doc.get_first(title).unwrap().as_str().unwrap()
        );
    }

    // Search for documents containing "document"
    let query = query_parser.parse_query("document")?;
    let top_docs = searcher.search(&query, &TopDocs::with_limit(2))?;

    println!("\nSearch results for 'document':");
    for (_score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
        println!(
            "Title: {}",
            retrieved_doc.get_first(title).unwrap().as_str().unwrap()
        );
    }

    Ok(())
}
