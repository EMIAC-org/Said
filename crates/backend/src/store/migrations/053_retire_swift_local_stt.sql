-- Retire the Python-sidecar Swift STT (Oriserve via PyTorch/transformers).
-- Existing users on `swift_local` are moved to the native, Python-free
-- whisper.cpp path (`whisper_local`). Deepgram (cloud) users are left untouched.
UPDATE preferences SET stt_provider = 'whisper_local' WHERE stt_provider = 'swift_local';
