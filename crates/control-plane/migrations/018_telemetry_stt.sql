-- Per-run STT provider metadata for datapoints analytics.

ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS stt_provider TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS stt_model TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS stt_path TEXT;
