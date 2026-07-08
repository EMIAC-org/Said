-- Force local polish preference to Cerebras Gemma 4.

UPDATE preferences
   SET selected_model = 'cerebras-gemma-4'
 WHERE selected_model IS NOT NULL
   AND selected_model != 'cerebras-gemma-4';
