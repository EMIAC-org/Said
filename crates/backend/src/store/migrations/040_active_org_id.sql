-- Multi-org: persist active workspace on the local user row.
ALTER TABLE local_user ADD COLUMN active_org_id TEXT;
