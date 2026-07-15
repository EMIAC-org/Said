-- Add the OpenRouter credential and force the one production route to paid
-- Gemma 4 31B through OpenRouter Nitro. The previous provider column remains
-- only as immutable SQLite migration history and is no longer read by code.
ALTER TABLE preferences ADD COLUMN openrouter_api_key TEXT;

UPDATE preferences
   SET selected_model = 'openrouter-gemma-4-nitro'
 WHERE selected_model IS NULL
    OR selected_model != 'openrouter-gemma-4-nitro';
