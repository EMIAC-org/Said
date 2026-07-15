-- The previous migration introduced Together only for cloud STT credentials.
-- Voice polish remains on the OpenRouter Nitro route.
UPDATE preferences
   SET selected_model = 'openrouter-gemma-4-nitro'
 WHERE selected_model IS NULL
    OR selected_model != 'openrouter-gemma-4-nitro';
