-- GPT OSS 20B is the production default polish model (was smart / 120B).

UPDATE preferences
   SET selected_model = 'groq-gpt-oss-20b'
 WHERE selected_model IN ('smart', 'openai/gpt-oss-120b');
