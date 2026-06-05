-- Option A: record the desktop build's OS version and build channel so the
-- admin fleet view can show which machines moved onto the wider macOS 11+
-- build (build_channel = 'standard') and which run the macOS 13+ echo
-- cancellation build (build_channel = 'echo'). Both nullable for backward
-- compatibility — older clients simply do not send them.
ALTER TABLE desktop_clients
    ADD COLUMN IF NOT EXISTS os_version TEXT;

ALTER TABLE desktop_clients
    ADD COLUMN IF NOT EXISTS build_channel TEXT;
