-- Lock dictation polish to Cerebras GPT OSS 120B.

UPDATE preferences
   SET selected_model = 'cerebras-gpt-oss'
 WHERE selected_model IS NOT NULL
   AND selected_model != 'cerebras-gpt-oss';

-- STT provider preference was retired in the local-only speech cleanup.
