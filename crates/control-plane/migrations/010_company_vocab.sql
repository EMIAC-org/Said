-- Company vocabulary buckets for enterprise installs.
-- Admins curate org-approved terms/aliases. Desktop clients upload summary
-- counts only, never raw transcripts or example sentences.

CREATE TABLE IF NOT EXISTS org_vocab_terms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    term        TEXT NOT NULL,
    term_norm   TEXT NOT NULL,
    term_type   TEXT NOT NULL DEFAULT 'other',
    language    TEXT NOT NULL DEFAULT 'hinglish',
    weight      DOUBLE PRECISION NOT NULL DEFAULT 1,
    priority    INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'draft',
    created_by  UUID REFERENCES accounts(id) ON DELETE SET NULL,
    updated_by  UUID REFERENCES accounts(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, term_norm)
);
CREATE INDEX IF NOT EXISTS idx_org_vocab_terms_org_status ON org_vocab_terms (org_id, status, priority DESC, updated_at DESC);

CREATE TABLE IF NOT EXISTS org_vocab_aliases (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    transcript_form TEXT NOT NULL,
    transcript_norm TEXT NOT NULL,
    correct_form    TEXT NOT NULL,
    correct_norm    TEXT NOT NULL,
    language        TEXT NOT NULL DEFAULT 'hinglish',
    weight          DOUBLE PRECISION NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'draft',
    safety_status   TEXT NOT NULL DEFAULT 'unknown',
    created_by      UUID REFERENCES accounts(id) ON DELETE SET NULL,
    updated_by      UUID REFERENCES accounts(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, transcript_norm, correct_norm)
);
CREATE INDEX IF NOT EXISTS idx_org_vocab_aliases_org_status ON org_vocab_aliases (org_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS org_vocab_releases (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    bucket_hash   TEXT NOT NULL,
    manifest_json JSONB NOT NULL,
    notes         TEXT,
    published_by  UUID REFERENCES accounts(id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, version)
);
CREATE INDEX IF NOT EXISTS idx_org_vocab_releases_org_version ON org_vocab_releases (org_id, version DESC);

CREATE TABLE IF NOT EXISTS org_user_vocab_items (
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    term            TEXT NOT NULL,
    term_norm       TEXT NOT NULL,
    term_type       TEXT NOT NULL DEFAULT 'other',
    source          TEXT NOT NULL DEFAULT 'local',
    weight          DOUBLE PRECISION NOT NULL DEFAULT 1,
    use_count       INTEGER NOT NULL DEFAULT 0,
    positive_count  INTEGER NOT NULL DEFAULT 0,
    negative_count  INTEGER NOT NULL DEFAULT 0,
    safety_status   TEXT NOT NULL DEFAULT 'unknown',
    first_seen_at   TIMESTAMPTZ,
    last_seen_at    TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, account_id, device_id, term_norm)
);
CREATE INDEX IF NOT EXISTS idx_org_user_vocab_items_org_term ON org_user_vocab_items (org_id, term_norm);
CREATE INDEX IF NOT EXISTS idx_org_user_vocab_items_account ON org_user_vocab_items (org_id, account_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS org_user_vocab_aliases (
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    transcript_form TEXT NOT NULL,
    transcript_norm TEXT NOT NULL,
    correct_form    TEXT NOT NULL,
    correct_norm    TEXT NOT NULL,
    weight          DOUBLE PRECISION NOT NULL DEFAULT 1,
    use_count       INTEGER NOT NULL DEFAULT 0,
    positive_count  INTEGER NOT NULL DEFAULT 0,
    negative_count  INTEGER NOT NULL DEFAULT 0,
    safety_status   TEXT NOT NULL DEFAULT 'unknown',
    review_status   TEXT NOT NULL DEFAULT 'unknown',
    first_seen_at   TIMESTAMPTZ,
    last_seen_at    TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, account_id, device_id, transcript_norm, correct_norm)
);
CREATE INDEX IF NOT EXISTS idx_org_user_vocab_aliases_org_pair ON org_user_vocab_aliases (org_id, transcript_norm, correct_norm);
CREATE INDEX IF NOT EXISTS idx_org_user_vocab_aliases_account ON org_user_vocab_aliases (org_id, account_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS org_vocab_suggestions (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    suggestion_key        TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    term                  TEXT,
    term_norm             TEXT,
    term_type             TEXT,
    transcript_form       TEXT,
    transcript_norm       TEXT,
    correct_form          TEXT,
    correct_norm          TEXT,
    users_count           INTEGER NOT NULL DEFAULT 0,
    total_positive_count  INTEGER NOT NULL DEFAULT 0,
    total_negative_count  INTEGER NOT NULL DEFAULT 0,
    confidence            DOUBLE PRECISION NOT NULL DEFAULT 0,
    safety_status         TEXT NOT NULL DEFAULT 'unknown',
    status                TEXT NOT NULL DEFAULT 'pending',
    reviewed_by           UUID REFERENCES accounts(id) ON DELETE SET NULL,
    reviewed_at           TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, suggestion_key)
);
CREATE INDEX IF NOT EXISTS idx_org_vocab_suggestions_org_status ON org_vocab_suggestions (org_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS org_vocab_audit_log (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    actor_id    UUID REFERENCES accounts(id) ON DELETE SET NULL,
    action      TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id   UUID,
    payload     JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_org_vocab_audit_log_org ON org_vocab_audit_log (org_id, created_at DESC);

ALTER TABLE desktop_clients ADD COLUMN IF NOT EXISTS company_bucket_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE desktop_clients ADD COLUMN IF NOT EXISTS company_vocab_synced_at TIMESTAMPTZ;
ALTER TABLE desktop_clients ADD COLUMN IF NOT EXISTS personal_vocab_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE desktop_clients ADD COLUMN IF NOT EXISTS personal_alias_count INTEGER NOT NULL DEFAULT 0;
