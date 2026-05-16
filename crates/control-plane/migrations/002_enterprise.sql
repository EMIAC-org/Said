-- Enterprise meeting system ---------------------------------------------------

-- Orgs -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS orgs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    slug            TEXT UNIQUE NOT NULL,
    lark_app_id     TEXT,
    lark_app_secret TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Org members ----------------------------------------------------------------
CREATE TABLE IF NOT EXISTS org_members (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role            TEXT NOT NULL DEFAULT 'member',  -- 'admin' | 'member'
    lark_user_id    TEXT,
    lark_name       TEXT,
    lark_avatar_url TEXT,
    lark_department TEXT,
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, account_id)
);
CREATE INDEX IF NOT EXISTS idx_org_members_org ON org_members (org_id);

-- Lark tokens ----------------------------------------------------------------
CREATE TABLE IF NOT EXISTS lark_tokens (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    access_token    TEXT NOT NULL,
    refresh_token   TEXT NOT NULL,
    token_expires_at TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, org_id)
);

-- Meetings -------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meetings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    agenda          TEXT,
    status          TEXT NOT NULL DEFAULT 'scheduled',  -- 'scheduled' | 'live' | 'ended'
    created_by      UUID NOT NULL REFERENCES accounts(id),
    started_at      TIMESTAMPTZ,
    ended_at        TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_meetings_org_status ON meetings (org_id, status);

-- Meeting participants -------------------------------------------------------
CREATE TABLE IF NOT EXISTS meeting_participants (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    status          TEXT NOT NULL DEFAULT 'invited',  -- 'invited' | 'joined' | 'left'
    joined_at       TIMESTAMPTZ,
    left_at         TIMESTAMPTZ,
    UNIQUE (meeting_id, account_id)
);
CREATE INDEX IF NOT EXISTS idx_meeting_participants_meeting ON meeting_participants (meeting_id);

-- Transcript chunks ----------------------------------------------------------
CREATE TABLE IF NOT EXISTS transcript_chunks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    speaker_id      UUID NOT NULL REFERENCES accounts(id),
    text            TEXT NOT NULL,
    chunk_index     INTEGER NOT NULL,
    timestamp_ms    BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_transcript_chunks_meeting ON transcript_chunks (meeting_id, chunk_index);

-- Meeting tasks (AI-extracted action items) ----------------------------------
CREATE TABLE IF NOT EXISTS meeting_tasks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    assignee_id     UUID REFERENCES accounts(id),
    due_date        TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'synced' | 'done'
    lark_task_id    TEXT,
    detected_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_meeting_tasks_meeting ON meeting_tasks (meeting_id);

-- Meeting decisions (AI-detected decisions) ----------------------------------
CREATE TABLE IF NOT EXISTS meeting_decisions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    text            TEXT NOT NULL,
    detected_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Meeting summaries (one per meeting, updated incrementally) -----------------
CREATE TABLE IF NOT EXISTS meeting_summaries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID NOT NULL UNIQUE REFERENCES meetings(id) ON DELETE CASCADE,
    summary_text    TEXT NOT NULL,
    lark_doc_id     TEXT,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
