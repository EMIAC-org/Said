-- Global polish switch.
--
-- Defaults to 1 so every existing user keeps the behaviour they have today.
-- Turning it off sends the local transcript straight to the paste path without
-- an LLM pass; the Devanagari→Roman guard still runs, so Hinglish output stays
-- Roman either way.
ALTER TABLE preferences ADD COLUMN polish_enabled INTEGER NOT NULL DEFAULT 1;
