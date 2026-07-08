-- Migration 004: Add API key columns to preferences
ALTER TABLE preferences ADD COLUMN gateway_api_key  TEXT;
ALTER TABLE preferences ADD COLUMN gemini_api_key   TEXT;
