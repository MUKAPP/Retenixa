use crate::{
    contracts::{DomainError, ErrorCode, Identifier, Sha256, Text},
    error::Result,
    maintenance, project, storage, util,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, TransactionBehavior, params, types::ValueRef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256 as Hasher};
use std::{fs::OpenOptions, path::Path};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMergeInput {
    pub workspace_path: Text,
    pub source_project_id: Identifier,
    pub target_project_id: Identifier,
    #[serde(default)]
    pub expected_plan_hash: Option<Sha256>,
}
const TABLES: &[&str] = &[
    "project_locations",
    "workspaces",
    "workflows",
    "contexts",
    "attachment_receipts",
    "tasks",
    "checkpoints",
    "knowledge",
    "knowledge_revisions",
    "evidence",
    "checkpoint_knowledge_refs",
    "checkpoint_evidence_refs",
    "events",
    "request_dedup",
    "search_documents",
];
const UNIQUE_KEYS: &[(&str, &str)] = &[
    ("workspaces", "s.id=t.id"),
    ("workflows", "s.id=t.id"),
    ("workflows", "s.binding_hash=t.binding_hash"),
    ("contexts", "s.id=t.id"),
    (
        "attachment_receipts",
        "s.binding_hash=t.binding_hash AND s.request_id=t.request_id",
    ),
    ("tasks", "s.id=t.id"),
    ("checkpoints", "s.id=t.id"),
    (
        "checkpoints",
        "s.task_id=t.task_id AND s.sequence=t.sequence",
    ),
    ("knowledge", "s.id=t.id"),
    ("knowledge", "s.key=t.key AND s.scope_json=t.scope_json"),
    ("knowledge_revisions", "s.id=t.id AND s.revision=t.revision"),
    ("evidence", "s.id=t.id"),
    (
        "checkpoint_knowledge_refs",
        "s.checkpoint_id=t.checkpoint_id AND s.object_id=t.object_id AND s.revision=t.revision",
    ),
    (
        "checkpoint_evidence_refs",
        "s.checkpoint_id=t.checkpoint_id AND s.object_id=t.object_id AND s.revision=t.revision",
    ),
    (
        "request_dedup",
        "s.workflow_id=t.workflow_id AND s.request_id=t.request_id",
    ),
    ("search_documents", "s.doc_id=t.doc_id"),
];
const IMMUTABLE: &[&str] = &[
    "checkpoint_immutable_update",
    "checkpoint_immutable_delete",
    "knowledge_revision_immutable_update",
    "knowledge_revision_immutable_delete",
    "evidence_immutable_update",
    "evidence_immutable_delete",
];
fn scope_error(message: impl Into<String>) -> DomainError {
    DomainError::new(ErrorCode::ProjectScopeMismatch, message)
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// Hash complete authoritative row content, including receipt hashes and historical JSON.
// Derived FTS internals are excluded; their source rows and row ids are included.
fn state_hash(
    conn: &Connection,
    input: &ProjectMergeInput,
    location: &project::Location,
) -> Result<String> {
    let mut hasher = Hasher::new();
    hasher.update(util::canonical(&json!({"source":input.source_project_id,"target":input.target_project_id,"root":location.root,"identity":location.identity}))?);
    let mut schema =
        conn.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name")?;
    let schema_rows = schema
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    hasher.update(serde_json::to_vec(&schema_rows)?);
    for (kind, name, _, _) in schema_rows {
        if kind != "table" || name.starts_with("search_fts") {
            continue;
        }
        let mut stmt = conn.prepare(&format!("SELECT * FROM {}", quoted(&name)))?;
        let columns = stmt.column_count();
        let mut rows = stmt.query([])?;
        let mut encoded = Vec::new();
        while let Some(row) = rows.next()? {
            let mut cells = Vec::new();
            for i in 0..columns {
                cells.push(match row.get_ref(i)? {
                    ValueRef::Null => json!(["null"]),
                    ValueRef::Integer(n) => json!(["integer", n.to_string()]),
                    ValueRef::Real(n) => json!(["real", n.to_bits().to_string()]),
                    ValueRef::Text(s) => json!(["text", s]),
                    ValueRef::Blob(s) => json!(["blob", s]),
                });
            }
            encoded.push(serde_json::to_vec(&cells)?);
        }
        encoded.sort();
        hasher.update(serde_json::to_vec(&(name, encoded))?);
    }
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    hasher.update(version.to_le_bytes());
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn plan(conn: &Connection, input: &ProjectMergeInput) -> Result<Value> {
    maintenance::verify(conn)?;
    let source = &input.source_project_id.0;
    let target = &input.target_project_id.0;
    if source == target {
        return Err(scope_error("source 与 target 必须不同"));
    }
    let location = project::locate(&input.workspace_path.0)?;
    if !location.git
        || std::fs::canonicalize(&input.workspace_path.0)? != Path::new(&location.root)
        || project::standard_main_root(&location)?.as_deref() != Some(&location.root)
    {
        return Err(scope_error(
            "project-merge 仅支持标准 Git 主 root 的精确旧 path 身份",
        ));
    }
    let old_identity = format!("path:{}", location.root);
    let alias: Option<String> = conn
        .query_row(
            "SELECT project_id FROM project_locations WHERE identity_key=?1",
            [&old_identity],
            |r| r.get(0),
        )
        .optional()?;
    let git_project: Option<String> = conn
        .query_row(
            "SELECT project_id FROM project_locations WHERE identity_key=?1",
            [&location.identity],
            |r| r.get(0),
        )
        .optional()?;
    if alias.as_deref() != Some(source) || git_project.as_deref() != Some(target) {
        return Err(scope_error(
            "旧 path alias/source 或当前 Git alias/target 不匹配",
        ));
    }
    let workspace: Option<String> = conn
        .query_row(
            "SELECT id FROM workspaces WHERE project_id=?1 AND root=?2 AND identity_key=?3",
            params![source, location.root, old_identity],
            |r| r.get(0),
        )
        .optional()?;
    let Some(workspace) = workspace else {
        return Err(scope_error(
            "source 必须持有当前主 root 的精确旧 path workspace",
        ));
    };
    let source_workspaces: i64 = conn.query_row(
        "SELECT count(*) FROM workspaces WHERE project_id=?1",
        [source],
        |r| r.get(0),
    )?;
    let other_aliases: i64 = conn.query_row(
        "SELECT count(*) FROM project_locations WHERE project_id=?1 AND identity_key!=?2",
        params![source, old_identity],
        |r| r.get(0),
    )?;
    if source_workspaces != 1 || other_aliases != 0 {
        return Err(scope_error(
            "source 已有其他 workspace/身份；此维护入口仅整合单一旧 path 项目",
        ));
    }
    let mut conflicts = Vec::new();
    for (table, key) in UNIQUE_KEYS {
        let sql = format!(
            "SELECT count(*) FROM {table} s JOIN {table} t ON {key} WHERE s.project_id=?1 AND t.project_id=?2"
        );
        let n: i64 = conn.query_row(&sql, params![source, target], |r| r.get(0))?;
        if n > 0 {
            conflicts.push(json!({"table":table,"constraint":key,"count":n}));
        }
    }
    let mut counts = serde_json::Map::new();
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for table in TABLES
        .iter()
        .copied()
        .chain((version == 3).then_some("workflow_host_aliases"))
    {
        let n: i64 = conn.query_row(
            &format!("SELECT count(*) FROM {table} WHERE project_id=?1"),
            [source],
            |r| r.get(0),
        )?;
        counts.insert(table.into(), json!(n));
    }
    let mut stmt=conn.prepare("SELECT s.id,s.host_identity FROM workflows s JOIN workflows t ON s.host_identity=t.host_identity WHERE s.project_id=?1 AND t.project_id=?2 ORDER BY s.id")?;
    let aliases = stmt
        .query_map(params![source, target], |r| {
            Ok(json!({"workflow_id":r.get::<_,String>(0)?,"host_identity":r.get::<_,String>(1)?}))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let hash = state_hash(conn, input, &location)?;
    Ok(
        json!({"mode":"plan","schema_version":version,"target_schema_version":storage::SCHEMA_VERSION,"source_project_id":source,"target_project_id":target,"workspace_id":workspace,"workspace_root":location.root,"old_identity":old_identity,"new_identity":location.identity,"row_counts":counts,"host_aliases":aliases,"conflicts":conflicts,"applicable":conflicts.is_empty(),"plan_hash":hash,"source_project_retained":true}),
    )
}

pub(crate) fn preserve_attachment_location(
    conn: &Connection,
    project: &str,
    workspace: &str,
    identity: &str,
) -> Result<()> {
    conn.execute("UPDATE attachment_receipts SET hash_location=coalesce(hash_location,?1) WHERE project_id=?2 AND json_extract(result_json,'$.workspace_id')=?3",params![identity,project,workspace])?;
    Ok(())
}

pub fn project_merge(root: &Path, input: ProjectMergeInput, confirmed: bool) -> Result<Value> {
    if !root.is_absolute() || !Path::new(&input.workspace_path.0).is_absolute() {
        return Err(DomainError::invalid(
            "data-dir 与 workspace_path 必须绝对路径",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    let db = root.join("retenixa.sqlite3");
    // The default path never opens Store, creates a lock, or runs migrations.
    if !confirmed {
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN")?;
        return plan(&conn, &input);
    }
    let gate = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("maintenance.lock"))?;
    gate.try_lock().map_err(|_| {
        DomainError::storage(
            "仍有 Retenixa 进程使用该数据目录；停止所有使用者后再执行 project-merge",
        )
    })?;
    let source = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.execute_batch("PRAGMA foreign_keys=ON; BEGIN")?;
    let before = plan(&source, &input)?;
    if before["applicable"] != true {
        return Err(scope_error(format!(
            "合并唯一约束冲突：{}",
            before["conflicts"]
        )));
    }
    if input
        .expected_plan_hash
        .as_ref()
        .is_some_and(|h| Some(h.0.as_str()) != before["plan_hash"].as_str())
    {
        return Err(DomainError::new(
            ErrorCode::VersionConflict,
            "plan 状态已变化；重新只读预演并审查",
        ));
    }
    let backup = root.join(format!("pre-project-merge-{}.sqlite3", util::id()));
    maintenance::new_backup(&source, &backup)?;
    source.execute_batch("ROLLBACK")?;
    drop(source);
    let mut conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    storage::configure(&conn)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = plan(&tx, &input)?;
    if current["plan_hash"] != before["plan_hash"] {
        return Err(DomainError::new(
            ErrorCode::VersionConflict,
            "备份后数据库发生变化；未执行合并",
        ));
    }
    tx.pragma_update(None, "defer_foreign_keys", true)?;
    storage::migrate_identity(&tx)?;
    let source = &input.source_project_id.0;
    let target = &input.target_project_id.0;
    let workspace = util::text(&before, "workspace_id");
    let old_identity = util::text(&before, "old_identity");
    preserve_attachment_location(&tx, source, &workspace, &old_identity)?;
    tx.execute("UPDATE request_dedup SET hash_project_id=coalesce(hash_project_id,project_id) WHERE project_id=?1",[source])?;
    // Archive only source hosts that would violate the target's unique constraint.
    tx.execute("INSERT INTO workflow_host_aliases(project_id,workflow_id,host_identity) SELECT s.project_id,s.id,s.host_identity FROM workflows s WHERE s.project_id=?1 AND s.host_identity IS NOT NULL AND EXISTS(SELECT 1 FROM workflows t WHERE t.project_id=?2 AND t.host_identity=s.host_identity)",params![source,target])?;
    tx.execute("UPDATE workflows SET host_identity=NULL WHERE project_id=?1 AND EXISTS(SELECT 1 FROM workflows t WHERE t.project_id=?2 AND t.host_identity=workflows.host_identity)",params![source,target])?;
    let mut triggers = Vec::new();
    for name in IMMUTABLE {
        let sql: String = tx.query_row(
            "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
            [name],
            |r| r.get(0),
        )?;
        triggers.push(sql);
        tx.execute_batch(&format!("DROP TRIGGER {}", quoted(name)))?;
    }
    for table in TABLES
        .iter()
        .copied()
        .chain(std::iter::once("workflow_host_aliases"))
    {
        tx.execute(
            &format!("UPDATE {table} SET project_id=?1 WHERE project_id=?2"),
            params![target, source],
        )?;
    }
    for sql in triggers {
        tx.execute_batch(&sql)?;
    }
    tx.execute(
        "UPDATE workspaces SET identity_key=?1 WHERE project_id=?2 AND id=?3",
        params![util::text(&before, "new_identity"), target, workspace],
    )?;
    // Only the receipt envelope carries the migrated namespace; nested semantic IDs stay intact.
    for table in ["attachment_receipts", "bootstrap_receipts"] {
        tx.execute(&format!("UPDATE {table} SET result_json=json_set(result_json,'$.project_id',?1) WHERE json_extract(result_json,'$.project_id')=?2"),params![target,source])?;
    }
    let audit = util::id();
    tx.execute("INSERT INTO project_merges(id,source_project_id,target_project_id,workspace_id,old_identity,new_identity,plan_hash,backup_path,merged_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![audit,source,target,workspace,old_identity,util::text(&before,"new_identity"),util::text(&before,"plan_hash"),backup.to_string_lossy(),util::now()])?;
    maintenance::verify(&tx)?;
    tx.commit()?;
    Ok(
        json!({"mode":"applied","audit_id":audit,"source_project_id":source,"target_project_id":target,"workspace_id":workspace,"plan_hash":before["plan_hash"],"backup_path":backup,"schema_version":storage::SCHEMA_VERSION,"integrity":"ok","source_project_retained":true,"reconnect_required":true}),
    )
}
