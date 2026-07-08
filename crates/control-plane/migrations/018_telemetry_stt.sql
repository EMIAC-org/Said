-- Per-run local speech model metadata for datapoints analytics.

ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_model TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_path TEXT;
