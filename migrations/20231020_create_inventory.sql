-- Migration file for creating the inventory management system
-- This file sets up the database schema for inventory items with full-text search capabilities

-- Create the main inventory_items table with all necessary columns
CREATE TABLE inventory_items (
    -- Basic item information
    id SERIAL PRIMARY KEY,                    -- Auto-incrementing unique identifier
    name VARCHAR(255) NOT NULL,               -- Item name, required, max 255 characters
    description TEXT,                         -- Detailed description, optional
    sku VARCHAR(50) UNIQUE NOT NULL,          -- Stock Keeping Unit, unique identifier for products
    quantity INTEGER NOT NULL DEFAULT 0,       -- Current stock quantity, defaults to 0
    price DECIMAL(10,2) NOT NULL,             -- Price with 2 decimal places
    category VARCHAR(100) NOT NULL,           -- Product category for organization

    -- Full-text search configuration
    -- Creates a tsvector column that combines name, description, and category
    -- with different weights (A being highest priority, C being lowest)
    search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(name, '')), 'A') ||        -- Name has highest search priority
        setweight(to_tsvector('english', coalesce(description, '')), 'B') || -- Description has medium priority
        setweight(to_tsvector('english', coalesce(category, '')), 'C')       -- Category has lowest priority
    ) STORED,                                 -- STORED means the column is physically stored, not virtual

    -- Timestamp fields for tracking record creation and updates
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,  -- When the record was created
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP   -- When the record was last modified
);

-- Create a GIN (Generalized Inverted Index) index for efficient full-text search
-- This improves search performance on the search_vector column
CREATE INDEX inventory_search_idx ON inventory_items USING GIN (search_vector);

-- Create a function to automatically update the updated_at timestamp
-- This function will be called by a trigger whenever a row is updated
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;    -- Set the updated_at to the current time
    RETURN NEW;                           -- Return the modified row
END;
$$ language 'plpgsql';

-- Create a trigger that calls the update_updated_at_column function
-- This trigger runs automatically BEFORE any UPDATE operation
CREATE TRIGGER update_inventory_modtime
    BEFORE UPDATE ON inventory_items
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
