-- GPT OSS 120B on Cerebras is the production default (no Groq 120B).

UPDATE preferences
   SET selected_model = 'cerebras-gpt-oss'
 WHERE selected_model IN ('smart', 'groq-gpt-oss-20b', 'openai/gpt-oss-120b');
