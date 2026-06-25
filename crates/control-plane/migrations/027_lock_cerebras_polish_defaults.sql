-- Force runtime settings to Cerebras GPT OSS 120B for all accounts.

UPDATE runtime_user_settings
   SET selected_model = 'cerebras-gpt-oss'
 WHERE selected_model IS NOT NULL
   AND selected_model != 'cerebras-gpt-oss';
