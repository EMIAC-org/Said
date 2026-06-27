-- DeepSeek was briefly a polish-model option; map any leftover rows to Groq fast.

UPDATE preferences
   SET selected_model = 'fast'
 WHERE selected_model = 'deepseek';
