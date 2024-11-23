# Full Text Search Engine

A high-performance, asynchronous full-text search engine built with Rust, leveraging the power of Tantivy for efficient text indexing and searching capabilities.

## 🚀 Features

- **Fast Full-Text Search**: Powered by Tantivy, offering blazing-fast search capabilities
- **RESTful API**: Built with Axum framework for robust HTTP endpoints
- **Async Architecture**: Fully asynchronous implementation using Tokio
- **Database Integration**: Seamless PostgreSQL integration using Sea-ORM
- **CORS Support**: Built-in Cross-Origin Resource Sharing support
- **Real-time Updates**: WebSocket support for real-time search updates
- **Structured Logging**: Comprehensive logging system using tracing

## 🛠️ Tech Stack

- **Rust** (Edition 2021)
- **Tantivy**: High-performance full-text search engine library
- **Axum**: Modern, fast web framework
- **Sea-ORM**: Async database ORM
- **PostgreSQL**: Primary database
- **Tokio**: Asynchronous runtime
- **Tower-HTTP**: HTTP middleware stack

## 📋 Prerequisites

- Rust (latest stable version)
- PostgreSQL
- Make (for running Makefile commands)

## 🚀 Getting Started

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd full_text_search
   ```

2. Set up environment variables:
   ```bash
   cp .env.example .env
   # Edit .env with your configuration
   ```

3. Set up the database:
   ```bash
   make migrate
   ```

4. Build and run the project:
   ```bash
   cargo run
   ```

## 🔧 Configuration

Configure the application through environment variables in the `.env` file:

- `DATABASE_URL`: PostgreSQL connection string
- Additional configuration variables...

## 📦 Project Structure

- `/src`: Source code
- `/migrations`: Database migration files
- `/index`: Search index storage
- `/tests`: Test files

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## ✨ Acknowledgments

- [Tantivy](https://github.com/quickwit-oss/tantivy) - The powerful search engine library
- [Axum](https://github.com/tokio-rs/axum) - Web framework
- [Sea-ORM](https://github.com/SeaQL/sea-orm) - Database ORM
