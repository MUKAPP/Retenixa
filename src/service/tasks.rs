use super::*;
use std::collections::HashSet;
impl Service {
    pub(super) fn create_task(&self, input: TaskCreateInput) -> Result<Value> {
        let payload = util::json(&input)?;
        self.mutate(&input.context_id.0,&input.request_id.0,"task_create",payload,|tx,a|{
        if let Some(parent)=&input.parent_task_id{task(tx,&a.project,&parent.0)?;}
        let id=util::id();let owner=if matches!(input.assignment,Assignment::CurrentWorkflow){Some(a.workflow.as_str())}else{None};
        tx.execute("INSERT INTO tasks(project_id,id,goal,scope,workspace_id,status,parent_task_id,owner_workflow_id,ownership_epoch,revision) VALUES(?1,?2,?3,?4,?7,'in_progress',?5,?6,1,1)",params![a.project,id,input.goal.0,input.scope.0,input.parent_task_id.as_ref().map(|p|&p.0),owner,a.workspace])?;
        Ok((json!({"task":task(tx,&a.project,&id)?}),vec![version(&id,1)]))})
    }
    pub(super) fn claim_task(&self, input: TaskClaimInput) -> Result<Value> {
        let payload = util::json(&input)?;
        self.mutate(&input.context_id.0,&input.request_id.0,"task_claim",payload,|tx,a|{
        let t=task(tx,&a.project,&input.task_id.0)?;
        if t["revision"].as_i64()!=Some(input.expected_revision.0){return Err(DomainError::new(ErrorCode::VersionConflict,"任务 revision 已变化"));}
        if t["ownership_epoch"].as_i64()!=Some(input.expected_epoch.0){return Err(DomainError::new(ErrorCode::OwnershipChanged,"任务 ownership_epoch 已变化"));}
        if matches!(t["status"].as_str(),Some("completed"|"abandoned")){return Err(DomainError::invalid("终态任务不能认领"));}
        if t["owner_workflow_id"].as_str()==Some(a.workflow.as_str()){return Ok((json!({"task":t}),vec![version(&input.task_id.0,input.expected_revision.0)]));}
        if !t["owner_workflow_id"].is_null(){return Err(DomainError::new(ErrorCode::ClaimDenied,"任务属于另一个 workflow；需要管理转交"));}
        let rev=next(input.expected_revision.0)?;let epoch=next(input.expected_epoch.0)?;
        let n=tx.execute("UPDATE tasks SET owner_workflow_id=?1,revision=?2,ownership_epoch=?3 WHERE project_id=?4 AND id=?5 AND revision=?6 AND ownership_epoch=?7 AND owner_workflow_id IS NULL",params![a.workflow,rev,epoch,a.project,input.task_id.0,input.expected_revision.0,input.expected_epoch.0])?;
        if n!=1{return Err(DomainError::new(ErrorCode::VersionConflict,"原子认领未命中"));}
        Ok((json!({"task":task(tx,&a.project,&input.task_id.0)?}),vec![version(&input.task_id.0,rev)]))})
    }
    pub(super) fn save_checkpoint(&self, input: CheckpointSaveInput) -> Result<Value> {
        let payload = util::json(&input)?;
        if let Some(receipt) = self.try_replay(
            &input.context_id.0,
            &input.request_id.0,
            "checkpoint_save",
            payload.clone(),
        )? {
            return Ok(receipt);
        }
        if input.snapshot.schema_version != 1 || input.snapshot.completion_conditions.is_empty() {
            return Err(DomainError::invalid(
                "快照 schema_version 必须 1 且完成条件非空",
            ));
        }
        if serde_json::to_vec(&input.snapshot)?.len() > 524288 {
            return Err(DomainError::invalid("完整快照超过 512KiB"));
        }
        if input.knowledge_refs.is_empty()
            && input.evidence_refs.is_empty()
            && input.snapshot.no_references_reason.is_none()
        {
            return Err(DomainError::invalid(
                "无固定依据时必须说明 no_references_reason",
            ));
        }
        let conn = self.store.connect()?;
        let access = self.access(&conn, &input.context_id.0, false)?;
        if input.snapshot.workspace.workspace_id.0 != access.workspace {
            return Err(DomainError::new(
                ErrorCode::ProjectScopeMismatch,
                "快照 workspace_id 与当前 context 不一致",
            ));
        }
        let service_observation =
            crate::project::observe(&access.root, input.snapshot.workspace.clone())?;
        drop(conn);
        let attempt_documents = crate::search::attempt_documents(
            "",
            "",
            &input.task_id.0,
            input.expected_revision.0 + 1,
            &input.snapshot,
        )?;
        self.mutate(&input.context_id.0,&input.request_id.0,"checkpoint_save",payload,|tx,a|{
            let t=task(tx,&a.project,&input.task_id.0)?;check_task(&t,a,input.expected_revision.0,input.ownership_epoch.0)?;
            let mut seen=HashSet::new();let mut warnings=Vec::new();
            for r in &input.knowledge_refs{if !seen.insert(format!("k:{}:{}",r.knowledge_id.0,r.revision.0)){return Err(DomainError::invalid("重复固定知识引用"));}let status=validate_knowledge_ref(tx,&a.project,r)?;if status!="active"{warnings.push(json!({"knowledge_id":r.knowledge_id,"current_status":status}));}}
            for r in &input.evidence_refs{if !seen.insert(format!("e:{}:{}",r.evidence_id.0,r.source_revision.0)){return Err(DomainError::invalid("重复固定证据引用"));}validate_evidence_ref(tx,&a.project,r)?;}
            for id in input.snapshot.verifications.iter().flat_map(|v|v.evidence_ids.iter()).chain(input.snapshot.attempts.iter().flat_map(|v|v.verification_refs.iter())){if !input.evidence_refs.iter().any(|r|r.evidence_id.0==id.0){return Err(DomainError::new(ErrorCode::ReferenceUnavailable,"验证/尝试引用须包含在精确 evidence_refs 中"));}}
            let rev=next(input.expected_revision.0)?;let checkpoint=util::id();let seq:i64=tx.query_row("SELECT coalesce(max(sequence),0)+1 FROM checkpoints WHERE project_id=?1 AND task_id=?2",params![a.project,input.task_id.0],|r|r.get(0))?;
            tx.execute("INSERT INTO checkpoints VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![a.project,checkpoint,input.task_id.0,seq,rev,a.workflow,input.ownership_epoch.0,serde_json::to_string(&input.snapshot)?,serde_json::to_string(&service_observation)?,util::now()])?;
            for r in &input.knowledge_refs{tx.execute("INSERT INTO checkpoint_knowledge_refs VALUES(?1,?2,?3,?4,?5)",params![a.project,checkpoint,r.knowledge_id.0,r.revision.0,serde_json::to_string(r)?])?;}
            for r in &input.evidence_refs{tx.execute("INSERT INTO checkpoint_evidence_refs VALUES(?1,?2,?3,?4,?5)",params![a.project,checkpoint,r.evidence_id.0,r.source_revision.0,serde_json::to_string(r)?])?;}
            let status=util::json(&input.target_status)?;let n=tx.execute("UPDATE tasks SET revision=?1,status=?2,latest_checkpoint_id=?3 WHERE project_id=?4 AND id=?5 AND revision=?6 AND ownership_epoch=?7 AND owner_workflow_id=?8",params![rev,status.as_str(),checkpoint,a.project,input.task_id.0,input.expected_revision.0,input.ownership_epoch.0,a.workflow])?;
            if n!=1{return Err(DomainError::new(ErrorCode::VersionConflict,"任务 CAS 未命中"));}
            for document in &attempt_documents {let mut d=document.clone();d.project=a.project.clone();d.entity=checkpoint.clone();d.id=format!("{checkpoint}:attempt:{}",d.body["attempt_index"].as_u64().unwrap());d.body["checkpoint_id"]=json!(checkpoint);crate::search::insert(tx,&d)?;}
            Ok((json!({"task":task(tx,&a.project,&input.task_id.0)?,"checkpoint_id":checkpoint,"sequence":seq,"workspace_observation":service_observation,"reference_warnings":warnings}),vec![version(&input.task_id.0,rev),version(&checkpoint,seq)]))
        })
    }
    pub(super) fn task_list(&self, input: TaskListInput) -> Result<Value> {
        if !(1..=100).contains(&input.limit) {
            return Err(DomainError::invalid("task_list.limit 范围 1—100"));
        }
        let mut db = self.store.connect()?;
        let tx = db.transaction()?;
        let conn = &tx;
        let a = self.access(conn, &input.context_id.0, false)?;
        if let Some(w) = &input.workspace_id {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE project_id=?1 AND id=?2)",
                params![a.project, w.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(DomainError::new(
                    ErrorCode::ProjectScopeMismatch,
                    "workspace 不属于当前项目",
                ));
            }
        }
        let filter = util::hash(util::canonical(
            &json!({"project":a.project,"status":input.status,"workspace":input.workspace_id,"parent":input.parent_task_id}),
        )?);
        let after = if let Some(c) = &input.cursor {
            let c = util::decode_cursor(c)?;
            if c["filter"].as_str() != Some(&filter) {
                return Err(DomainError::invalid("cursor 与当前项目/查询不匹配"));
            }
            util::text(&c, "after")
        } else {
            String::new()
        };
        let status = util::json(&input.status)?;
        let mut q=conn.prepare("SELECT t.id FROM tasks t WHERE t.project_id=?1 AND t.id>?2 AND (?3 IS NULL OR t.status=?3) AND (?4 IS NULL OR t.parent_task_id=?4) AND (?5 IS NULL OR t.workspace_id=?5) ORDER BY t.id LIMIT ?6")?;
        let ids = q
            .query_map(
                params![
                    a.project,
                    after,
                    status.as_str(),
                    input.parent_task_id.as_ref().map(|p| &p.0),
                    input.workspace_id.as_ref().map(|w| &w.0),
                    input.limit + 1
                ],
                |r| r.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = ids.len() > input.limit as usize;
        let visible = &ids[..ids.len().min(input.limit as usize)];
        let values = visible
            .iter()
            .map(|id| task(conn, &a.project, id))
            .collect::<Result<Vec<_>>>()?;
        let cursor = if more {
            Some(util::cursor(
                &json!({"filter":filter,"after":visible.last()}),
            )?)
        } else {
            None
        };
        Ok(json!({"tasks":values,"has_more":more,"next_cursor":cursor}))
    }
}
fn next(n: i64) -> Result<i64> {
    if n >= 9007199254740991 {
        return Err(DomainError::invalid("版本计数已到上限"));
    }
    Ok(n + 1)
}
pub(super) use super::knowledge::{validate_evidence_ref, validate_knowledge_ref};
