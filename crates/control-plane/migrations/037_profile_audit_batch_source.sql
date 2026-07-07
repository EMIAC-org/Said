-- Allow the batch profiling worker's audit source.
--
-- The per-app profiling batch (profile/updater/batch_run.rs apply_global_kb)
-- persists the global knowledge base — background, domains, focus areas — with
-- source = 'batch'. The audit-log source CHECK (migration 024) did not include
-- 'batch', so every KB upsert threw ("violates check constraint
-- runtime_profile_audit_log_source_check") and the learned DOMAINS were never
-- persisted. That silently starved the dynamic domain-context feature.
ALTER TABLE runtime_profile_audit_log
    DROP CONSTRAINT IF EXISTS runtime_profile_audit_log_source_check;

ALTER TABLE runtime_profile_audit_log
    ADD CONSTRAINT runtime_profile_audit_log_source_check
    CHECK (source IN (
        'api', 'rebuild_worker', 'migration', 'admin',
        'deepseek_edit', 'validator', 'batch'
    ));
