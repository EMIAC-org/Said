ALTER TABLE runtime_history_items
    ADD COLUMN IF NOT EXISTS dictation_trace_json JSONB NOT NULL DEFAULT '{}'::jsonb;
