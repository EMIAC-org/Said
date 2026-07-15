-- Keep an optional Together credential for cloud STT (Whisper Large v3 and
-- Nemotron). It is not a voice-polish provider.
ALTER TABLE preferences ADD COLUMN together_api_key TEXT;
