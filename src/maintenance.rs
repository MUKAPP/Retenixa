use crate::{
    contracts::{DomainError, ErrorCode},
    error::Result,
    permissions,
    storage::{APPLICATION_ID, SCHEMA_VERSION, Store},
    util,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, TransactionBehavior,
    backup::{Backup, StepResult},
    params,
};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    path::Path,
    time::{Duration, Instant},
};
pub(crate) fn verify(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    if !(2..=SCHEMA_VERSION).contains(&version) || app != APPLICATION_ID {
        return Err(DomainError::new(
            ErrorCode::SchemaUnsupported,
            "备份/数据库 schema 不兼容",
        ));
    }
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(DomainError::new(
            ErrorCode::StorageFailure,
            "SQLite 完整性检查失败",
        ));
    }
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    if stmt.query([])?.next()?.is_some() {
        return Err(DomainError::new(
            ErrorCode::ReferenceUnavailable,
            "数据库外键完整性检查失败",
        ));
    }
    Ok(())
}
fn copy(src: &Connection, dest: &mut Connection) -> Result<()> {
    let backup = Backup::new(src, dest)?;
    let start = Instant::now();
    loop {
        match backup.step(128)? {
            StepResult::Done => return Ok(()),
            StepResult::More => (),
            StepResult::Busy | StepResult::Locked => std::thread::sleep(Duration::from_millis(10)),
            _ => return Err(DomainError::new(ErrorCode::StorageFailure, "未知备份状态")),
        }
        if start.elapsed() > Duration::from_secs(15) {
            return Err(DomainError::storage(
                "一致性备份超过 15 秒边界，未继续无限等待",
            ));
        }
    }
}
pub(crate) fn new_backup(src: &Connection, path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(DomainError::invalid("备份输出必须绝对路径"));
    }
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    permissions::secure_file(path)?;
    drop(file);
    let mut dest = Connection::open(path)?;
    copy(src, &mut dest)?;
    verify(&dest)?;
    drop(dest);
    Ok(())
}
pub fn backup(store: &Store, path: &Path) -> Result<Value> {
    let source = store.connect()?;
    source.execute_batch("BEGIN")?;
    crate::storage::assert_schema(&source)?;
    new_backup(&source, path)?;
    Ok(json!({"backup_path":path,"schema_version":SCHEMA_VERSION,"consistent":true}))
}
pub fn restore(root: &Path, source_path: &Path) -> Result<Value> {
    if !root.is_absolute() || !source_path.is_absolute() {
        return Err(DomainError::invalid("恢复路径必须绝对路径"));
    }
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.execute_batch("BEGIN")?;
    verify(&source)?;
    permissions::secure_dir(root)?;
    let root = std::fs::canonicalize(root)?;
    let gate_path = root.join("maintenance.lock");
    let gate = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&gate_path)?;
    permissions::secure_file(&gate_path)?;
    gate.try_lock()
        .map_err(|_| DomainError::storage("仍有 Retenixa 进程使用该数据目录；先停止所有使用者"))?;
    let path = root.join("retenixa.sqlite3");
    if path.exists() && std::fs::canonicalize(&path)? == std::fs::canonicalize(source_path)? {
        return Err(DomainError::invalid("不能从数据库自身恢复自身"));
    }
    let mut previous = None;
    if path.exists() {
        let old = Connection::open(&path)?;
        let before = root.join(format!("pre-restore-{}.sqlite3", util::id()));
        new_backup(&old, &before)?;
        previous = Some(before);
    }
    let mut target = Connection::open(&path)?;
    permissions::secure_file(&path)?;
    copy(&source, &mut target)?;
    verify(&target)?;
    target.pragma_update(None, "journal_mode", "WAL")?;
    target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let restored_version: i64 = target.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(
        json!({"restored_from":source_path,"pre_restore_backup":previous,"schema_version":restored_version,"integrity":"ok","reconnect_required":true}),
    )
}
pub fn doctor(root: &Path) -> Result<Value> {
    let engine = crate::diagnostics::inspect(root.to_path_buf())
        .map_err(|e| DomainError::new(ErrorCode::StorageFailure, e.to_string()))?;
    let mut out = util::json(&engine)?;
    let path = root.join("retenixa.sqlite3");
    if !path.is_file() {
        return Ok(out);
    }
    let gate = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("maintenance.lock"))?;
    gate.try_lock_shared()
        .map_err(|_| DomainError::storage("数据目录正在排他维护"))?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_millis(750))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch("BEGIN")?;
    verify(&conn)?;
    out["persistent_database_initialized"] = json!(true);
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    out["schema_version"] = json!(version);
    out["integrity"] = json!("ok");
    let journal: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    out["journal_mode"] = json!(journal);
    let mut stmt =
        conn.prepare("SELECT project_id,id,generation FROM workflows ORDER BY project_id,id")?;
    let generations=stmt.query_map([],|r|Ok(json!({"project_id":r.get::<_,String>(0)?,"workflow_id":r.get::<_,String>(1)?,"writer_generation":r.get::<_,i64>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    out["workflows"] = json!(generations);
    let gaps:i64=conn.query_row("SELECT (SELECT count(*) FROM checkpoint_knowledge_refs r LEFT JOIN knowledge_revisions k ON r.project_id=k.project_id AND r.object_id=k.id AND r.revision=k.revision WHERE k.claim IS NULL)+(SELECT count(*) FROM checkpoint_evidence_refs r LEFT JOIN evidence e ON r.project_id=e.project_id AND r.object_id=e.id AND r.revision=e.revision WHERE e.observation IS NULL)",[],|r|r.get(0))?;
    out["reference_gaps"] = json!(gaps);
    let indexed: Option<String> = conn
        .query_row(
            "SELECT value FROM search_meta WHERE key='tokenizer_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    out["tokenizer_version"] = json!(indexed);
    out["index_compatible"] = json!(indexed.as_deref() == Some(crate::search::TOKENIZER_VERSION));
    Ok(out)
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferInput {
    pub project_id: crate::contracts::Identifier,
    pub task_id: crate::contracts::Identifier,
    pub target_workflow_id: crate::contracts::Identifier,
    pub expected_revision: crate::contracts::Revision,
    pub expected_epoch: crate::contracts::Revision,
    pub request_id: crate::contracts::Identifier,
    pub reason: crate::contracts::Text,
}
pub fn transfer(store: &Store, input: TransferInput, confirmed: bool) -> Result<Value> {
    if !confirmed {
        return Err(DomainError::new(
            ErrorCode::ClaimDenied,
            "任务转交需要本地管理确认",
        ));
    }
    let hash = util::hash(util::canonical(&util::json(&input)?)?);
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::storage::assert_schema(&tx)?;
    let merged_target: Option<String> = tx.query_row(
        "SELECT target_project_id FROM project_merges WHERE source_project_id=?1 ORDER BY merged_at DESC,id DESC LIMIT 1",
        [&input.project_id.0], |r| r.get(0),
    ).optional()?;
    let receipt_project = merged_target.as_deref().unwrap_or(&input.project_id.0);
    if let Some((h, body, origin)) = tx.query_row(
        "SELECT payload_hash,result_json,hash_project_id FROM request_dedup WHERE project_id=?1 AND workflow_id=?2 AND request_id=?3",
        params![receipt_project,input.target_workflow_id.0,input.request_id.0],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)),
    ).optional()? {
        if merged_target.is_some() && origin.as_deref() != Some(&input.project_id.0) {
            return Err(DomainError::new(ErrorCode::ProjectScopeMismatch,"原项目命名空间不属于该管理回执"));
        }
        let mut payload=util::json(&input)?;
        if let Some(origin)=origin { payload["project_id"]=json!(origin); }
        let replay_hash=util::hash(util::canonical(&payload)?);
        if h!=replay_hash { return Err(DomainError::new(ErrorCode::RequestIdReused,"管理请求 ID 用于不同载荷")); }
        let Some(body)=body else { return Ok(json!({"status":"result_purged","request_id":input.request_id,"reason":"原管理回执已清除"})); };
        let mut v:Value=serde_json::from_str(&body)?;v["replayed"]=json!(true);return Ok(v);
    }
    if merged_target.is_some() {
        return Err(DomainError::new(
            ErrorCode::ProjectScopeMismatch,
            "旧项目命名空间仅可确认迁移前的管理回执；新转交使用当前项目 ID",
        ));
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM workflows WHERE project_id=?1 AND id=?2)",
        params![input.project_id.0, input.target_workflow_id.0],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(DomainError::new(
            ErrorCode::ProjectScopeMismatch,
            "目标 workflow 不属于该项目",
        ));
    }
    let (rev, epoch, status): (i64, i64, String) = tx
        .query_row(
            "SELECT revision,ownership_epoch,status FROM tasks WHERE project_id=?1 AND id=?2",
            params![input.project_id.0, input.task_id.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| DomainError::new(ErrorCode::NotFound, "任务不存在"))?;
    if rev != input.expected_revision.0 {
        return Err(DomainError::new(
            ErrorCode::VersionConflict,
            "管理转交 revision 冲突",
        ));
    }
    if epoch != input.expected_epoch.0 {
        return Err(DomainError::new(
            ErrorCode::OwnershipChanged,
            "管理转交 epoch 冲突",
        ));
    }
    if matches!(status.as_str(), "completed" | "abandoned")
        || rev >= 9007199254740991
        || epoch >= 9007199254740991
    {
        return Err(DomainError::invalid("终态或版本上限不允许转交"));
    }
    let n=tx.execute("UPDATE tasks SET owner_workflow_id=?1,revision=?2,ownership_epoch=?3 WHERE project_id=?4 AND id=?5 AND revision=?6 AND ownership_epoch=?7",params![input.target_workflow_id.0,rev+1,epoch+1,input.project_id.0,input.task_id.0,rev,epoch])?;
    if n != 1 {
        return Err(DomainError::new(
            ErrorCode::VersionConflict,
            "管理转交 CAS 未命中",
        ));
    }
    let at = util::now();
    let versions = json!([{"entity_id":input.task_id,"revision":rev+1}]);
    let result = json!({"task_id":input.task_id,"revision":rev+1,"ownership_epoch":epoch+1,"owner_workflow_id":input.target_workflow_id});
    let receipt = json!({"status":"committed","request_id":input.request_id,"committed_at":at,"committed_versions":versions,"result":result,"replayed":false});
    tx.execute("INSERT INTO events(project_id,entity_id,revision,actor,request_id,kind,data_json,created_at) VALUES(?1,?2,?3,'local_management',?4,'task_transfer',?5,?6)",params![input.project_id.0,input.task_id.0,rev+1,input.request_id.0,serde_json::to_string(&input)?,at])?;
    tx.execute(
        "INSERT INTO request_dedup(project_id,workflow_id,request_id,payload_hash,result_json,committed_at,versions_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            input.project_id.0,
            input.target_workflow_id.0,
            input.request_id.0,
            hash,
            serde_json::to_string(&receipt)?,
            at,
            serde_json::to_string(&versions)?
        ],
    )?;
    tx.commit()?;
    Ok(receipt)
}
