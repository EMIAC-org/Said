-- Per-run local speech model metadata for datapoints analytics.

ALTER TABLE telemetry_run_summaries ADD COLUMN speech_model TEXT;
ALTER TABLE telemetry_run_summaries ADD COLUMN speech_path TEXT;
