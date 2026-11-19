use super::*;
impl Service {
    pub(super) fn resume(&self, input: ContextResumeInput) -> Result<Value> {
        let cursor = input
            .cursor
            .as_ref()
            .map(|c| util::decode_cursor(c))
            .transpose()?;
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        let a = self.access(&tx, &input.context_id.0, false)?;
        let t = task(&tx, &a.project, &input.task_id.0)?;
        let checkpoint = input
            .checkpoint_id
            .as_ref()
            .map(|x| x.0.clone())
            .or_else(|| {
                cursor
                    .as_ref()
                    .and_then(|c| c["checkpoint"].as_str().map(str::to_owned))
            })
            .or_else(|| t["latest_checkpoint_id"].as_str().map(str::to_owned))
            .ok_or_else(|| DomainError::new(ErrorCode::NotFound, "任务尚未保存检查点"))?;
        let (seq,revision,owner,epoch,raw,service_raw):(i64,i64,Option<String>,i64,String,String)=tx.query_row("SELECT sequence,task_revision,owner_workflow_id,ownership_epoch,snapshot_json,service_observation_json FROM checkpoints WHERE project_id=?1 AND task_id=?2 AND id=?3",params![a.project,input.task_id.0,checkpoint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?.ok_or_else(||DomainError::new(ErrorCode::NotFound,"该任务没有选定检查点"))?;
        let snapshot: CheckpointSnapshot = serde_json::from_str(&raw)?;
        let service_workspace_observation: WorkspaceObservation =
            serde_json::from_str(&service_raw)?;
        let mut baseline = service_workspace_observation.clone();
        if matches!(snapshot.workspace.head, Observed::Known { .. }) {
            baseline.head = snapshot.workspace.head.clone();
        }
        if matches!(snapshot.workspace.branch, Observed::Known { .. }) {
            baseline.branch = snapshot.workspace.branch.clone();
        }
        for (saved, reported) in baseline
            .related_files
            .iter_mut()
            .zip(&snapshot.workspace.related_files)
        {
            if matches!(reported.sha256, Observed::Known { .. }) {
                saved.sha256 = reported.sha256.clone();
            }
        }
        let knowledge_refs = ref_rows::<KnowledgeReference>(
            &tx,
            "checkpoint_knowledge_refs",
            &a.project,
            &checkpoint,
        )?;
        let evidence_refs = ref_rows::<EvidenceReference>(
            &tx,
            "checkpoint_evidence_refs",
            &a.project,
            &checkpoint,
        )?;
        let page_signature = util::hash(util::canonical(
            &json!({"project":a.project,"task":input.task_id,"checkpoint":checkpoint,"refs":knowledge_refs,"evidence":evidence_refs}),
        )?);
        let offset = if let Some(c) = &cursor {
            if c["filter"].as_str() != Some(&page_signature)
                || c["checkpoint"].as_str() != Some(checkpoint.as_str())
            {
                return Err(DomainError::invalid(
                    "恢复 cursor 与项目、任务或选定检查点不一致",
                ));
            }
            c["offset"]
                .as_u64()
                .filter(|n| *n <= 2000)
                .ok_or_else(|| DomainError::invalid("恢复 cursor 位置无效"))? as usize
        } else {
            0
        };
        let mut fixed_knowledge = Vec::new();
        let mut fixed_evidence = Vec::new();
        let mut delta = Vec::new();
        for r in &knowledge_refs {
            let row=tx.query_row("SELECT v.claim,v.content_hash,v.status,k.current_revision,k.status FROM knowledge_revisions v LEFT JOIN knowledge k ON v.project_id=k.project_id AND v.id=k.id WHERE v.project_id=?1 AND v.id=?2 AND v.revision=?3",params![a.project,r.knowledge_id.0,r.revision.0],|q|Ok((q.get::<_,Option<String>>(0)?,q.get::<_,String>(1)?,q.get::<_,String>(2)?,q.get::<_,Option<i64>>(3)?,q.get::<_,Option<String>>(4)?))).optional()?;
            match row {
                None => delta.push(change(
                    DeltaKind::Missing,
                    &r.knowledge_id.0,
                    Some(r.revision.0),
                    None,
                    "固定知识版本不存在",
                )),
                Some((None, _, _, rev, _)) => delta.push(change(
                    DeltaKind::Purged,
                    &r.knowledge_id.0,
                    Some(r.revision.0),
                    rev,
                    "固定知识正文已清除",
                )),
                Some((Some(claim), hash, status, current, current_status)) => {
                    if util::hash(&claim) != hash
                        || r.content_sha256.as_ref().is_some_and(|h| h.0 != hash)
                    {
                        delta.push(change(
                            DeltaKind::Unknown,
                            &r.knowledge_id.0,
                            Some(r.revision.0),
                            current,
                            "固定版本哈希不一致，正文不能作为完整依据",
                        ));
                        continue;
                    }
                    fixed_knowledge.push(FixedKnowledge {
                        knowledge_id: r.knowledge_id.clone(),
                        revision: r.revision.clone(),
                        claim: Text(claim),
                        content_sha256: Sha256(hash),
                        status: serde_json::from_value(json!(status))?,
                    });
                    if current_status.as_deref() == Some("retracted") {
                        delta.push(change(
                            DeltaKind::Retracted,
                            &r.knowledge_id.0,
                            Some(r.revision.0),
                            current,
                            "当前知识已撤回；历史正文保留但须复核",
                        ));
                    } else if current_status.as_deref() == Some("disputed") {
                        delta.push(change(
                            DeltaKind::Disputed,
                            &r.knowledge_id.0,
                            Some(r.revision.0),
                            current,
                            "当前知识处于争议状态",
                        ));
                    } else if current != Some(r.revision.0) {
                        delta.push(change(
                            if current.is_none() {
                                DeltaKind::Missing
                            } else {
                                DeltaKind::Changed
                            },
                            &r.knowledge_id.0,
                            Some(r.revision.0),
                            current,
                            &format!(
                                "当前知识版本/状态已变化，状态={}",
                                current_status.unwrap_or_else(|| "missing".into())
                            ),
                        ));
                    }
                }
            }
        }
        for r in &evidence_refs {
            let row=tx.query_row("SELECT source_json,observation,content_hash FROM evidence WHERE project_id=?1 AND id=?2 AND revision=?3",params![a.project,r.evidence_id.0,r.source_revision.0],|q|Ok((q.get::<_,String>(0)?,q.get::<_,Option<String>>(1)?,q.get::<_,String>(2)?))).optional()?;
            match row {
                None => delta.push(change(
                    DeltaKind::Missing,
                    &r.evidence_id.0,
                    Some(r.source_revision.0),
                    None,
                    "固定证据不存在",
                )),
                Some((source, observation, hash)) => {
                    if hash != r.content_sha256.0 {
                        delta.push(change(
                            DeltaKind::Unknown,
                            &r.evidence_id.0,
                            Some(r.source_revision.0),
                            Some(r.source_revision.0),
                            "证据内容哈希不一致",
                        ));
                    } else {
                        fixed_evidence.push(serde_json::from_str::<FixedEvidence>(&source)?);
                        if observation.is_none() {
                            delta.push(change(
                                DeltaKind::Purged,
                                &r.evidence_id.0,
                                Some(r.source_revision.0),
                                Some(r.source_revision.0),
                                "证据正文已清除，保留来源元数据",
                            ));
                        }
                    }
                }
            }
        }
        if t["owner_workflow_id"] != json!(owner) || t["ownership_epoch"].as_i64() != Some(epoch) {
            delta.push(change(
                DeltaKind::OwnershipChanged,
                &input.task_id.0,
                Some(revision),
                t["revision"].as_i64(),
                "当前任务所有者/epoch 与检查点不同；读取不会取得写权",
            ));
        } else if t["revision"].as_i64() != Some(revision) {
            delta.push(change(
                DeltaKind::Changed,
                &input.task_id.0,
                Some(revision),
                t["revision"].as_i64(),
                "当前任务已在选定检查点之后更新",
            ));
        }
        let saved_root = tx
            .query_row(
                "SELECT root FROM workspaces WHERE project_id=?1 AND id=?2",
                params![a.project, snapshot.workspace.workspace_id.0],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let current_task = CurrentTask {
            task_id: input.task_id.clone(),
            revision: Revision(t["revision"].as_i64().unwrap()),
            ownership_epoch: Revision(t["ownership_epoch"].as_i64().unwrap()),
            owner_workflow_id: t["owner_workflow_id"]
                .as_str()
                .map(|s| Identifier(s.into())),
            status: serde_json::from_value(t["status"].clone())?,
        };
        tx.commit()?;
        let current_observation = if let Some(root) = saved_root {
            match crate::project::observe(&root, service_workspace_observation.clone()) {
                Ok(observed) => {
                    if util::json(&observed.head)? != util::json(&baseline.head)?
                        || util::json(&observed.branch)? != util::json(&baseline.branch)?
                    {
                        delta.push(change(
                            DeltaKind::Changed,
                            &snapshot.workspace.workspace_id.0,
                            None,
                            None,
                            "Git HEAD 或分支与保存时不同",
                        ));
                    }
                    for (old, current) in baseline.related_files.iter().zip(&observed.related_files)
                    {
                        if matches!(current.sha256, Observed::Unknown { .. })
                            || util::json(&old.sha256)? != util::json(&current.sha256)?
                        {
                            delta.push(change(
                                if matches!(current.sha256, Observed::Unknown { .. }) {
                                    DeltaKind::Unknown
                                } else {
                                    DeltaKind::Changed
                                },
                                &format!("file:{}", util::hash(&old.path.0)),
                                None,
                                None,
                                &format!("相关文件变化或不可读：{}", old.path.0),
                            ));
                        }
                    }
                    Observed::Known { value: observed }
                }
                Err(e) => {
                    delta.push(change(
                        DeltaKind::Unknown,
                        &snapshot.workspace.workspace_id.0,
                        None,
                        None,
                        "保存工作区当前无法观测",
                    ));
                    Observed::Unknown { reason: e.message }
                }
            }
        } else {
            delta.push(change(
                DeltaKind::Missing,
                &snapshot.workspace.workspace_id.0,
                None,
                None,
                "保存工作区绑定不存在",
            ));
            Observed::Unknown {
                reason: Text("保存工作区绑定不存在".into()),
            }
        };
        if a.workspace != snapshot.workspace.workspace_id.0 {
            delta.push(change(
                DeltaKind::Changed,
                &a.workspace,
                None,
                None,
                "当前连接位于另一个 worktree；恢复未切换文件写入位置",
            ));
        }
        let mut requirements: Vec<ResumeRequirement> = delta
            .iter()
            .map(|d| ResumeRequirement {
                subject_id: d.subject_id.clone(),
                action: Text(format!("复核：{}", d.reason.0)),
                available_evidence: Vec::new(),
            })
            .collect();
        for op in &snapshot.unresolved_operations {
            requirements.push(ResumeRequirement {
                subject_id: op.operation_id.clone(),
                action: Text(format!(
                    "使用宿主 {} 核查结果；不要盲目重执行。{}",
                    op.host.0, op.verification_method.0
                )),
                available_evidence: Vec::new(),
            });
        }
        if current_task
            .owner_workflow_id
            .as_ref()
            .is_none_or(|w| w.0 != a.workflow)
        {
            requirements.push(ResumeRequirement {
                subject_id: input.task_id.clone(),
                action: Text("当前 workflow 没有该任务写权；认领无主任务或经明确管理转交".into()),
                available_evidence: Vec::new(),
            });
        }
        let (related, related_more) = self.related_knowledge(&input.context_id.0, &a, &snapshot)?;
        let mut package = ResumePackage {
            schema_version: 1,
            returned_at: Timestamp(util::now()),
            saved_context: SavedContext {
                checkpoint_id: Identifier(checkpoint.clone()),
                checkpoint_sequence: Revision(seq),
                task_revision_at_save: Revision(revision),
                snapshot,
                service_workspace_observation,
                knowledge_refs,
                evidence_refs,
                fixed_knowledge,
                fixed_evidence,
            },
            current_task,
            current_workspace_observation: current_observation,
            current_delta: delta,
            resume_requirements: requirements,
            related_context: Vec::new(),
            fixed_content_page: FixedContentPage {
                offset: offset as u32,
                returned: 0,
                total: 0,
            },
            omitted_categories: Vec::new(),
            has_more: false,
            next_cursor: None,
        };
        let knowledge = std::mem::take(&mut package.saved_context.fixed_knowledge);
        let evidence = std::mem::take(&mut package.saved_context.fixed_evidence);
        let total = knowledge.len() + evidence.len();
        if offset > total {
            return Err(DomainError::invalid(
                "恢复 cursor 越界，正文可用性已变化，请刷新恢复",
            ));
        }
        package.fixed_content_page.total = total as u32;
        set_page_state(&mut package, offset, total, &page_signature, &checkpoint)?;
        let mut complete = package.clone();
        complete.saved_context.fixed_knowledge = knowledge.iter().skip(offset).cloned().collect();
        complete.saved_context.fixed_evidence = evidence
            .iter()
            .skip(offset.saturating_sub(knowledge.len()))
            .cloned()
            .collect();
        set_page_state(&mut complete, total, total, &page_signature, &checkpoint)?;
        if related.is_empty() && !related_more {
            complete
                .omitted_categories
                .retain(|x| !x.0.starts_with("related_context"));
        }
        let mut end = offset;
        if util::budget(&complete, input.byte_budget, Some(checkpoint.clone())).is_ok() {
            package = complete;
            end = total;
        } else {
            util::budget(&package, input.byte_budget, Some(checkpoint.clone()))?;
            for at in offset..total {
                if at < knowledge.len() {
                    package
                        .saved_context
                        .fixed_knowledge
                        .push(knowledge[at].clone());
                } else {
                    package
                        .saved_context
                        .fixed_evidence
                        .push(evidence[at - knowledge.len()].clone());
                }
                set_page_state(&mut package, at + 1, total, &page_signature, &checkpoint)?;
                if let Err(error) =
                    util::budget(&package, input.byte_budget, Some(checkpoint.clone()))
                {
                    if at < knowledge.len() {
                        package.saved_context.fixed_knowledge.pop();
                    } else {
                        package.saved_context.fixed_evidence.pop();
                    }
                    set_page_state(&mut package, at, total, &page_signature, &checkpoint)?;
                    if at == offset {
                        return Err(error);
                    }
                    break;
                }
                end = at + 1;
            }
        }
        if end == total {
            let related_total = related.len();
            for item in related {
                package.related_context.push(item);
                if util::budget(&package, input.byte_budget, Some(checkpoint.clone())).is_err() {
                    package.related_context.pop();
                    break;
                }
            }
            if package.related_context.len() == related_total && !related_more {
                package
                    .omitted_categories
                    .retain(|x| !x.0.starts_with("related_context"));
            }
        }
        util::json(&util::budget(package, input.byte_budget, Some(checkpoint))?)
    }
    fn related_knowledge(
        &self,
        context: &str,
        a: &Access,
        snapshot: &CheckpointSnapshot,
    ) -> Result<(Vec<FixedKnowledge>, bool)> {
        let response = self.memory_search(MemorySearchInput {
            context_id: Identifier(context.into()),
            query: snapshot.goal.0.chars().take(256).collect(),
            kind: SearchKind::Knowledge,
            scope: SearchScope {
                task_id: None,
                workspace_id: Some(Identifier(a.workspace.clone())),
                path_prefix: snapshot
                    .workspace
                    .related_files
                    .first()
                    .map(|f| f.path.clone()),
                include_inactive: false,
                conditions: Vec::new(),
            },
            cursor: None,
            byte_budget: 65536,
            limit: 5,
        });
        let response = match response {
            Ok(r) => r,
            Err(e) if matches!(e.code, ErrorCode::BudgetTooSmall | ErrorCode::InvalidInput) => {
                return Ok((Vec::new(), true));
            }
            Err(e) => return Err(e),
        };
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        self.access(&tx, context, false)?;
        let mut related = Vec::new();
        for candidate in response["results"].as_array().into_iter().flatten() {
            if candidate["applicability"] != "match" {
                continue;
            }
            let id = candidate["entity_id"].as_str().unwrap_or("");
            let revision = candidate["revision"].as_i64().unwrap_or(0);
            let record=tx.query_row("SELECT v.claim,v.content_hash FROM knowledge k JOIN knowledge_revisions v ON k.project_id=v.project_id AND k.id=v.id AND k.current_revision=v.revision WHERE k.project_id=?1 AND k.id=?2 AND k.current_revision=?3 AND k.status='active'",params![a.project,id,revision],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,String>(1)?))).optional()?;
            if let Some((Some(claim), hash)) = record {
                related.push(FixedKnowledge {
                    knowledge_id: Identifier(id.into()),
                    revision: Revision(revision),
                    claim: Text(claim),
                    content_sha256: Sha256(hash),
                    status: KnowledgeStatus::Active,
                });
            }
        }
        tx.commit()?;
        Ok((
            related,
            response["has_more"].as_bool().unwrap_or(false)
                || response["scan_limit_reached"].as_bool().unwrap_or(false),
        ))
    }
    pub(super) fn memory_get(&self, input: MemoryGetInput) -> Result<Value> {
        if input.ids.is_empty() || input.ids.len() > 100 {
            return Err(DomainError::invalid("memory_get 需要 1—100 个 ID"));
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        let a = self.access(&tx, &input.context_id.0, false)?;
        let filter = util::hash(util::canonical(
            &json!({"project":a.project,"ids":input.ids,"revision":input.revision}),
        )?);
        let offset = if let Some(c) = &input.cursor {
            let c = util::decode_cursor(c)?;
            if c["filter"].as_str() != Some(&filter) {
                return Err(DomainError::invalid("cursor 与当前 memory_get 不匹配"));
            }
            c["offset"]
                .as_i64()
                .ok_or_else(|| DomainError::invalid("cursor 缺 offset"))? as usize
        } else {
            0
        };
        if offset > input.ids.len() {
            return Err(DomainError::invalid("cursor 越界"));
        }
        let mut records = Vec::new();
        let mut at = offset;
        while at < input.ids.len() {
            let record = memory_record(
                &tx,
                &a.project,
                &input.ids[at].0,
                input.revision.as_ref().map(|r| r.0),
            )?;
            records.push(record);
            let more = at + 1 < input.ids.len();
            let next = if more {
                Some(util::cursor(&json!({"filter":filter,"offset":at+1}))?)
            } else {
                None
            };
            let page = json!({"records":records,"has_more":more,"next_cursor":next});
            if let Err(e) = util::budget(&page, input.byte_budget, Some(input.ids[at].0.clone())) {
                records.pop();
                if records.is_empty() {
                    return Err(e);
                }
                let next = util::cursor(&json!({"filter":filter,"offset":at}))?;
                let out = util::budget(
                    json!({"records":records,"has_more":true,"next_cursor":next}),
                    input.byte_budget,
                    Some(input.ids[at].0.clone()),
                )?;
                tx.commit()?;
                return Ok(out);
            }
            at += 1;
        }
        let out = util::budget(
            json!({"records":records,"has_more":false,"next_cursor":null}),
            input.byte_budget,
            None,
        )?;
        tx.commit()?;
        Ok(out)
    }
}
fn set_page_state(
    package: &mut ResumePackage,
    end: usize,
    total: usize,
    signature: &str,
    checkpoint: &str,
) -> Result<()> {
    package.fixed_content_page.returned = end as u32 - package.fixed_content_page.offset;
    package.has_more = end < total;
    package.next_cursor = if end < total {
        Some(util::cursor(
            &json!({"filter":signature,"checkpoint":checkpoint,"offset":end}),
        )?)
    } else {
        None
    };
    package.omitted_categories = if end < total {
        vec![Text(format!(
            "fixed_reference_bodies：共 {total} 项，本页从 {} 返回 {} 项，其余经 context_resume.next_cursor 读取；完整快照、变化摘要和未知操作均保留",
            package.fixed_content_page.offset, package.fixed_content_page.returned
        ))]
    } else {
        Vec::new()
    };
    package.omitted_categories.push(Text("related_context：补充知识按预算或结果上限省略，可显式 memory_search/memory_get；未自动采用为固定依据".into()));
    Ok(())
}
fn change(
    kind: DeltaKind,
    id: &str,
    saved: Option<i64>,
    current: Option<i64>,
    reason: &str,
) -> ContextDelta {
    ContextDelta {
        kind,
        subject_id: Identifier(id.into()),
        saved_revision: saved.map(Revision),
        current_revision: current.map(Revision),
        reason: Text(reason.into()),
        applicability: Applicability::NeedsReview,
    }
}
fn ref_rows<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    table: &str,
    project: &str,
    id: &str,
) -> Result<Vec<T>> {
    let mut s=conn.prepare(&format!("SELECT ref_json FROM {table} WHERE project_id=?1 AND checkpoint_id=?2 ORDER BY object_id,revision"))?;
    let rows = s
        .query_map(params![project, id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.iter().map(|r| Ok(serde_json::from_str(r)?)).collect()
}
fn memory_record(
    conn: &Connection,
    project: &str,
    id: &str,
    revision: Option<i64>,
) -> Result<Value> {
    if let Some((r,claim,hash,status,meta))=conn.query_row("SELECT v.revision,v.claim,v.content_hash,v.status,v.metadata_json FROM knowledge_revisions v JOIN knowledge k ON v.project_id=k.project_id AND v.id=k.id WHERE v.project_id=?1 AND v.id=?2 AND v.revision=coalesce(?3,k.current_revision)",params![project,id,revision],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional()?{return Ok(json!({"kind":"knowledge","id":id,"revision":r,"availability":if claim.is_some(){"available"}else{"purged"},"claim":claim,"content_sha256":hash,"status":status,"metadata":serde_json::from_str::<Value>(&meta)?}));}
    if let Some((source,observation))=conn.query_row("SELECT source_json,observation FROM evidence WHERE project_id=?1 AND id=?2 AND revision=coalesce(?3,1)",params![project,id,revision],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional()?{return Ok(json!({"kind":"evidence","id":id,"availability":if observation.is_some(){"available"}else{"purged"},"source":serde_json::from_str::<Value>(&source)?,"observation":observation}));}
    if let Some((raw,sequence,task_id,observed))=conn.query_row("SELECT snapshot_json,sequence,task_id,service_observation_json FROM checkpoints WHERE project_id=?1 AND id=?2 AND (?3 IS NULL OR sequence=?3)",params![project,id,revision],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?{return Ok(json!({"kind":"checkpoint","id":id,"sequence":sequence,"task_id":task_id,"snapshot":serde_json::from_str::<Value>(&raw)?,"service_workspace_observation":serde_json::from_str::<Value>(&observed)?,"knowledge_refs":ref_rows::<KnowledgeReference>(conn,"checkpoint_knowledge_refs",project,id)?,"evidence_refs":ref_rows::<EvidenceReference>(conn,"checkpoint_evidence_refs",project,id)?}));}
    Ok(json!({"id":id,"availability":"missing"}))
}
