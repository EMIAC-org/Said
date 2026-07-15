-- Preserve the provider that actually produced each transcript. The selected
-- local model is not a reliable proxy when cloud STT is active.
ALTER TABLE telemetry_run_summaries ADD COLUMN speech_provider TEXT;
