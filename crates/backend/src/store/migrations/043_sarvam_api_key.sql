-- Per-user Sarvam API key (mirrors deepgram_api_key storage).

ALTER TABLE preferences ADD COLUMN sarvam_api_key TEXT;
