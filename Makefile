include .env
export

# PostgreSQL service status check
.PHONY: pg_status

pg_status:
	@echo Checking PostgreSQL service status...
	@sc query postgresql-x64-16 | findstr "STATE" | findstr "RUNNING" > nul && echo PostgreSQL service is running || (echo Error: PostgreSQL service is not running && exit 1)

# Build the project
.PHONY: build

build:
	@echo Building the project...
	@cargo build --release

# Run the project
.PHONY: run

run: pg_status
	@echo Running the project...
	@cargo run --release

# Clean build artifacts
.PHONY: clean

clean:
	@echo Cleaning build artifacts...
	@cargo clean

.PHONY: all
all: build run

.DEFAULT_GOAL := all
