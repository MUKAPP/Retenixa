ALTER TABLE request_dedup ADD COLUMN hash_project_id TEXT;
ALTER TABLE attachment_receipts ADD COLUMN hash_location TEXT;
CREATE TABLE workflow_host_aliases(project_id TEXT NOT NULL,workflow_id TEXT NOT NULL,host_identity TEXT NOT NULL,PRIMARY KEY(project_id,workflow_id,host_identity),FOREIGN KEY(project_id,workflow_id) REFERENCES workflows(project_id,id));
CREATE INDEX workflow_host_alias_lookup ON workflow_host_aliases(project_id,host_identity);
CREATE TABLE project_merges(id TEXT PRIMARY KEY,source_project_id TEXT NOT NULL REFERENCES projects(id),target_project_id TEXT NOT NULL REFERENCES projects(id),workspace_id TEXT NOT NULL,old_identity TEXT NOT NULL,new_identity TEXT NOT NULL,plan_hash TEXT NOT NULL,backup_path TEXT NOT NULL,merged_at TEXT NOT NULL);
