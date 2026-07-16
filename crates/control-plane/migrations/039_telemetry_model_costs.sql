-- Forward-only compatibility repair. Production historically used stt_* names
-- while newer binaries queried speech_*. Keep one canonical speech_* contract
-- and backfill without directly referencing columns that may not exist.
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_provider TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_model TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_path TEXT;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_cost_usd DOUBLE PRECISION;
ALTER TABLE runtime_telemetry_runs ADD COLUMN IF NOT EXISTS speech_cost_source TEXT;

UPDATE runtime_telemetry_runs
   SET speech_provider = COALESCE(
           NULLIF(speech_provider, ''),
           NULLIF(to_jsonb(runtime_telemetry_runs) ->> 'stt_provider', ''),
           CASE
             WHEN COALESCE(speech_model, to_jsonb(runtime_telemetry_runs) ->> 'stt_model', '') LIKE 'together:%' THEN 'together'
             WHEN COALESCE(speech_model, to_jsonb(runtime_telemetry_runs) ->> 'stt_model', '') LIKE 'local:%' THEN 'local_nemotron'
             WHEN COALESCE(speech_path, to_jsonb(runtime_telemetry_runs) ->> 'stt_path', '') LIKE 'local%' THEN 'local_whisper'
             ELSE NULL
           END
       ),
       speech_model = COALESCE(
           NULLIF(speech_model, ''),
           NULLIF(to_jsonb(runtime_telemetry_runs) ->> 'stt_model', '')
       ),
       speech_path = COALESCE(
           NULLIF(speech_path, ''),
           NULLIF(to_jsonb(runtime_telemetry_runs) ->> 'stt_path', '')
       );

UPDATE runtime_telemetry_runs
   SET speech_cost_usd = CASE
           WHEN speech_provider = 'together'
                AND (speech_model ILIKE '%nemotron%' OR speech_path ILIKE '%websocket%')
             THEN COALESCE(audio_seconds, 0) * 0.09 / 3600.0
           WHEN speech_provider IN ('local', 'local_whisper', 'local_nemotron', 'swift_local', 'whisper_local')
             THEN 0.0
           ELSE speech_cost_usd
       END,
       speech_cost_source = CASE
           WHEN speech_provider = 'together'
                AND (speech_model ILIKE '%nemotron%' OR speech_path ILIKE '%websocket%')
             THEN 'rate:together_nemotron_0.09_per_hour@2026-07-15'
           WHEN speech_provider IN ('local', 'local_whisper', 'local_nemotron', 'swift_local', 'whisper_local')
             THEN 'local_zero'
           ELSE speech_cost_source
       END
 WHERE speech_cost_usd IS NULL;

ALTER TABLE runtime_provider_usage ADD COLUMN IF NOT EXISTS generation_id TEXT;
ALTER TABLE runtime_provider_usage ADD COLUMN IF NOT EXISTS cost_source TEXT;
ALTER TABLE runtime_provider_usage ADD COLUMN IF NOT EXISTS usage_json JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS idx_runtime_telemetry_speech_provider_date
    ON runtime_telemetry_runs (org_id, speech_provider, event_at DESC);
CREATE INDEX IF NOT EXISTS idx_runtime_provider_usage_generation
    ON runtime_provider_usage (generation_id)
    WHERE generation_id IS NOT NULL;
