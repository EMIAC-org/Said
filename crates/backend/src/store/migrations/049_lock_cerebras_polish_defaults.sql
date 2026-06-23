-- Lock dictation polish to Cerebras GPT OSS 120B; Turbo Q5 is meetings-only.

UPDATE preferences
   SET selected_model = 'cerebras-gpt-oss'
 WHERE selected_model IS NOT NULL
   AND selected_model != 'cerebras-gpt-oss';

UPDATE preferences
   SET stt_provider = 'deepgram'
 WHERE stt_provider = 'whisper_local';
