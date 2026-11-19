ALTER TABLE search_documents ADD COLUMN attempt_hash TEXT;
CREATE INDEX search_attempt_identity_idx
    ON search_documents(project_id, task_id, attempt_hash, doc_id)
    WHERE attempt_hash IS NOT NULL;

-- Populate fingerprints from the authoritative snapshots before serving search.
DELETE FROM search_meta WHERE key='tokenizer_version';
