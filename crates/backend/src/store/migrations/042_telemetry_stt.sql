-- Per-run STT provider metadata for datapoints analytics.

ALTER TABLE telemetry_run_summaries ADD COLUMN stt_provider TEXT;
ALTER TABLE telemetry_run_summaries ADD COLUMN stt_model TEXT;
ALTER TABLE telemetry_run_summaries ADD COLUMN stt_path TEXT;
