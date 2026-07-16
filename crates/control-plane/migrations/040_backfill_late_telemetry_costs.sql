-- Migration 039 may have run before a desktop telemetry backlog arrived. Infer
-- identity and cost again for those late rows so retries and historical admin
-- views converge on the same attribution as live ingest.
UPDATE runtime_telemetry_runs
   SET speech_provider = CASE
           WHEN NULLIF(speech_provider, '') IS NOT NULL THEN speech_provider
           WHEN COALESCE(speech_model, '') ILIKE 'together:%'
                OR COALESCE(speech_path, '') ILIKE 'websocket%'
             THEN 'together'
           WHEN COALESCE(speech_model, '') ILIKE 'local:%nemotron%'
             THEN 'local_nemotron'
           WHEN COALESCE(speech_path, '') ILIKE 'local%'
             THEN 'local_whisper'
           ELSE speech_provider
       END;

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
