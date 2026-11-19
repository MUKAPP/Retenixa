mod context;
mod knowledge;
mod resume;
mod retrieval;
mod tasks;
use crate::{contracts::*, error::Result, storage::Store, util};
pub use context::HostAttachment;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Clone)]
pub struct Service {
    pub store: Store,
    connection_id: String,
    host: Option<Arc<HostAttachment>>,
    stdio_process: Option<crate::process_identity::ProcessIdentity>,
}
#[derive(Clone)]
struct Access {
    project: String,
    workflow: String,
    workspace: String,
    root: String,
}
impl Service {
    pub fn new(store: Store, host: Option<HostAttachment>) -> Self {
        Self {
            store,
            connection_id: util::id(),
            host: host.map(Arc::new),
            stdio_process: None,
        }
    }
    pub(crate) fn for_stdio(store: Store, host: Option<HostAttachment>) -> Self {
        let mut service = Self::new(store, host);
        if service.host.is_none() {
            service.stdio_process = crate::process_identity::ProcessIdentity::current();
        }
        service
    }
    pub fn call(&self, op: &str, input: Value) -> Result<Value> {
        util::validate_tree(&input, 0)?;
        if serde_json::to_vec(&input)?.len() > 1_048_576 {
            return Err(DomainError::invalid("请求超过 1MiB"));
        }
        match op {
            "context_open" => self.open_context(util::parse(input)?),
            "task_create" => self.create_task(util::parse(input)?),
            "task_claim" => self.claim_task(util::parse(input)?),
            "task_release" => self.release_task(util::parse(input)?),
            "checkpoint_save" => self.save_checkpoint(util::parse(input)?),
            "knowledge_write" => self.write_knowledge(util::parse(input)?),
            "context_resume" => self.resume(util::parse(input)?),
            "request_get" => self.request_get(util::parse(input)?),
            "memory_get" => self.memory_get(util::parse(input)?),
            "memory_search" => self.memory_search(util::parse(input)?),
            "task_list" => self.task_list(util::parse(input)?),
            _ => Err(DomainError::invalid("工具未在当前阶段实现")),
        }
    }
    fn access(&self, conn: &Connection, context: &str, write: bool) -> Result<Access> {
        crate::storage::assert_schema(conn)?;
        crate::search::assert_index(conn)?;
        let row=conn.query_row("SELECT c.project_id,c.workflow_id,c.workspace_id,w.root,c.generation,f.generation,f.active_context_id FROM contexts c JOIN workflows f ON f.project_id=c.project_id AND f.id=c.workflow_id JOIN workspaces w ON w.project_id=c.project_id AND w.id=c.workspace_id WHERE c.id=?1 AND c.connection_id=?2",params![context,self.connection_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?))).optional()?;
        let Some((project, workflow, workspace, root, generation, current, active)) = row else {
            return Err(DomainError::new(
                ErrorCode::StaleAttachment,
                "context 不属于当前传输连接",
            ));
        };
        if write && (generation != current || active != context) {
            return Err(DomainError::new(
                ErrorCode::StaleAttachment,
                "当前附着已过期",
            ));
        }
        Ok(Access {
            project,
            workflow,
            workspace,
            root,
        })
    }
    fn mutate<F>(
        &self,
        context: &str,
        request: &str,
        op: &str,
        mut payload: Value,
        write: F,
    ) -> Result<Value>
    where
        F: FnOnce(&Transaction<'_>, &Access) -> Result<(Value, Vec<EntityVersion>)>,
    {
        if let Some(o) = payload.as_object_mut() {
            o.remove("context_id");
            o.remove("request_id");
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let a = self.access(&tx, context, true)?;
        let hash = util::hash(util::canonical(
            &json!({"schema_version":1,"op":op,"project_id":a.project,"workflow_id":a.workflow,"payload":payload}),
        )?);
        if let Some((old_hash,body,origin))=tx.query_row("SELECT payload_hash,result_json,hash_project_id FROM request_dedup WHERE project_id=?1 AND workflow_id=?2 AND request_id=?3",params![a.project,a.workflow,request],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()? {
            let replay_hash = util::hash(util::canonical(&json!({"schema_version":1,"op":op,"project_id":origin.as_deref().unwrap_or(&a.project),"workflow_id":a.workflow,"payload":payload}))?);
            if old_hash!=replay_hash{return Err(DomainError::new(ErrorCode::RequestIdReused,"请求 ID 已经用于不同语义载荷"));}
            let Some(body)=body else{return Ok(json!({"status":"result_purged","request_id":request,"reason":"已提交的回执正文已清除；不会重新执行"}));};
            let mut result:Value=serde_json::from_str(&body)?;result["replayed"]=json!(true);return Ok(result);
        }
        let (result, versions) = write(&tx, &a)?;
        let at = util::now();
        for v in &versions {
            tx.execute("INSERT INTO events(project_id,entity_id,revision,actor,request_id,kind,data_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![a.project,v.entity_id.0,v.revision.0,a.workflow,request,op,serde_json::to_string(&result)?,at])?;
        }
        let receipt = json!({"status":"committed","request_id":request,"committed_at":at,"committed_versions":versions,"result":result,"replayed":false});
        tx.execute(
            "INSERT INTO request_dedup(project_id,workflow_id,request_id,payload_hash,result_json,committed_at,versions_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                a.project,
                a.workflow,
                request,
                hash,
                serde_json::to_string(&receipt)?,
                at,
                serde_json::to_string(&versions)?
            ],
        )?;
        tx.commit()?;
        Ok(receipt)
    }
    fn try_replay(
        &self,
        context: &str,
        request: &str,
        op: &str,
        mut payload: Value,
    ) -> Result<Option<Value>> {
        if let Some(o) = payload.as_object_mut() {
            o.remove("context_id");
            o.remove("request_id");
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        let a = self.access(&tx, context, true)?;
        let row=tx.query_row("SELECT payload_hash,result_json,hash_project_id FROM request_dedup WHERE project_id=?1 AND workflow_id=?2 AND request_id=?3",params![a.project,a.workflow,request],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
        let result = if let Some((h, body, origin)) = row {
            let hash = util::hash(util::canonical(
                &json!({"schema_version":1,"op":op,"project_id":origin.as_deref().unwrap_or(&a.project),"workflow_id":a.workflow,"payload":payload}),
            )?);
            if h != hash {
                return Err(DomainError::new(
                    ErrorCode::RequestIdReused,
                    "请求 ID 已经用于不同载荷",
                ));
            }
            Some(if let Some(body) = body {
                let mut v: Value = serde_json::from_str(&body)?;
                v["replayed"] = json!(true);
                v
            } else {
                json!({"status":"result_purged","request_id":request,"reason":"已提交回执正文已清除，不重新执行"})
            })
        } else {
            None
        };
        tx.commit()?;
        Ok(result)
    }
    fn request_get(&self, input: RequestGetInput) -> Result<Value> {
        let mut db = self.store.connect()?;
        let tx = db.transaction()?;
        let conn = &tx;
        let a = self.access(conn, &input.context_id.0, false)?;
        let body=conn.query_row("SELECT result_json FROM request_dedup WHERE project_id=?1 AND workflow_id=?2 AND request_id=?3",params![a.project,a.workflow,input.request_id.0],|r|r.get::<_,Option<String>>(0)).optional()?;
        match body {
            Some(Some(b)) => Ok(serde_json::from_str(&b)?),
            Some(None) => Ok(
                json!({"status":"result_purged","request_id":input.request_id,"reason":"已提交结果正文不可提供"}),
            ),
            None => Ok(json!({"status":"not_found","request_id":input.request_id})),
        }
    }
}
fn version(id: &str, n: i64) -> EntityVersion {
    EntityVersion {
        entity_id: Identifier(id.into()),
        revision: Revision(n),
    }
}
fn task(conn: &Connection, project: &str, id: &str) -> Result<Value> {
    conn.query_row("SELECT revision,ownership_epoch,owner_workflow_id,status,goal,scope,parent_task_id,latest_checkpoint_id FROM tasks WHERE project_id=?1 AND id=?2",params![project,id],|r|Ok(json!({"task_id":id,"revision":r.get::<_,i64>(0)?,"ownership_epoch":r.get::<_,i64>(1)?,"owner_workflow_id":r.get::<_,Option<String>>(2)?,"status":r.get::<_,String>(3)?,"goal":r.get::<_,String>(4)?,"scope":r.get::<_,String>(5)?,"parent_task_id":r.get::<_,Option<String>>(6)?,"latest_checkpoint_id":r.get::<_,Option<String>>(7)?}))).optional()?.ok_or_else(||DomainError::new(ErrorCode::NotFound,"当前项目不存在该任务"))
}
fn check_task(t: &Value, a: &Access, rev: i64, epoch: i64) -> Result<()> {
    if t["ownership_epoch"].as_i64() != Some(epoch)
        || t["owner_workflow_id"].as_str() != Some(a.workflow.as_str())
    {
        return Err(task_ownership_error(t, "任务归属/所有权代次不匹配"));
    }
    if t["revision"].as_i64() != Some(rev) {
        return Err(DomainError::new(
            ErrorCode::VersionConflict,
            "任务 revision 已变化",
        ));
    }
    if matches!(t["status"].as_str(), Some("completed" | "abandoned")) {
        return Err(DomainError::invalid("终态任务不能隐式重开"));
    }
    Ok(())
}

fn task_ownership_error(t: &Value, message: &str) -> DomainError {
    DomainError::ownership_changed(
        message,
        t["task_id"].as_str().unwrap(),
        t["revision"].as_i64().unwrap(),
        t["ownership_epoch"].as_i64().unwrap(),
        t["owner_workflow_id"].as_str(),
    )
}
