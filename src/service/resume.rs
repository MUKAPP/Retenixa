use super::*;
impl Service {
    pub(super) fn resume(&self, input: ContextResumeInput) -> Result<Value> {
        util::validate_budget(input.byte_budget)?;
        if input.view == ResumeView::Compact && input.cursor.is_some() {
            return Err(DomainError::invalid("compact 视图不支持固定正文分页"));
        }
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
        let page_signature = util::hash(util::canonical(&json!({
            "project":a.project,"task":input.task_id,"checkpoint":checkpoint,"view":input.view,
            "knowledge":fixed_knowledge.iter().map(|r| (&r.knowledge_id,&r.revision)).collect::<Vec<_>>(),
            "evidence":fixed_evidence.iter().map(|r| (&r.evidence_id,&r.source_revision)).collect::<Vec<_>>()
        }))?);
        let offset = if let Some(c) = &cursor {
            if c["filter"].as_str() != Some(&page_signature)
                || c["checkpoint"].as_str() != Some(checkpoint.as_str())
            {
                return Err(DomainError::invalid(
                    "恢复 cursor 不匹配或固定正文可用性已变化，请刷新恢复",
                ));
            }
            c["offset"]
                .as_u64()
                .filter(|n| *n <= 2000)
                .ok_or_else(|| DomainError::invalid("恢复 cursor 位置无效"))? as usize
        } else {
            0
        };
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
                action: Text("当前 workflow 没有该任务写权；按归属使用 task_claim，或由原所有者释放、操作者转交".into()),
                available_evidence: Vec::new(),
            });
        }
        let limit = match input.view {
            ResumeView::Compact => 0,
            ResumeView::Standard => 2,
            ResumeView::Full => 5,
        };
        let (related, related_more) = if limit == 0 || offset != 0 {
            (Vec::new(), offset == 0)
        } else {
            self.related_knowledge(&input.context_id.0, &a, &snapshot, &knowledge_refs, limit)?
        };
        let package = ResumePackage {
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
            view: input.view,
            related_context: related,
            fixed_content_page: FixedContentPage {
                offset: offset as u32,
                returned: 0,
                total: 0,
            },
            omitted_sections: Vec::new(),
            omitted_categories: Vec::new(),
            has_more: false,
            next_cursor: None,
        };
        let package = fit_resume(package, related_more, &page_signature, input.byte_budget)?;
        util::json(&util::budget(package, input.byte_budget, Some(checkpoint))?)
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
            let page = MemoryGetOutput {
                records: &records,
                has_more: more,
                next_cursor: next.as_deref(),
            };
            if let Err(e) = util::budget(&page, input.byte_budget, Some(input.ids[at].0.clone())) {
                records.pop();
                if records.is_empty() {
                    return Err(e);
                }
                let next = util::cursor(&json!({"filter":filter,"offset":at}))?;
                let out = util::budget(
                    MemoryGetOutput {
                        records: &records,
                        has_more: true,
                        next_cursor: Some(&next),
                    },
                    input.byte_budget,
                    Some(input.ids[at].0.clone()),
                )?;
                tx.commit()?;
                return util::json(&out);
            }
            at += 1;
        }
        let out = util::budget(
            MemoryGetOutput {
                records: &records,
                has_more: false,
                next_cursor: None,
            },
            input.byte_budget,
            None,
        )?;
        tx.commit()?;
        util::json(&out)
    }
}
fn fit_resume(
    mut p: ResumePackage,
    related_more: bool,
    signature: &str,
    budget: u32,
) -> Result<ResumePackage> {
    let fits = |p: &ResumePackage| Ok::<_, DomainError>(util::payload_bytes(p)? <= budget as usize);
    let mut knowledge = std::mem::take(&mut p.saved_context.fixed_knowledge);
    let mut evidence = std::mem::take(&mut p.saved_context.fixed_evidence);
    let total = knowledge.len() + evidence.len();
    let offset = p.fixed_content_page.offset as usize;
    if offset > total {
        return Err(DomainError::invalid(
            "恢复 cursor 越界，正文可用性已变化，请刷新恢复",
        ));
    }
    p.fixed_content_page.total = total as u32;
    drop(evidence.drain(..offset.saturating_sub(knowledge.len())));
    drop(knowledge.drain(..offset.min(knowledge.len())));
    if related_more {
        omit(&mut p, ResumeSection::RelatedContext);
    }
    if p.view != ResumeView::Compact {
        p.fixed_content_page.returned = (total - offset) as u32;
        p.saved_context.fixed_knowledge = knowledge;
        p.saved_context.fixed_evidence = evidence;
        if fits(&p)? {
            return Ok(p);
        }
        if !p.related_context.is_empty() {
            p.related_context.clear();
            if !related_more {
                omit(&mut p, ResumeSection::RelatedContext);
            }
            if fits(&p)? {
                return Ok(p);
            }
        }
        knowledge = std::mem::take(&mut p.saved_context.fixed_knowledge);
        evidence = std::mem::take(&mut p.saved_context.fixed_evidence);
    }
    p.fixed_content_page.returned = 0;
    if offset < total {
        omit(&mut p, ResumeSection::FixedReferenceBodies);
    }
    if p.view != ResumeView::Compact && offset + 1 < total {
        // The full page has already been tried. Partial pages share the same omission and cursor overhead.
        let mut low = 0;
        let mut high = total - offset;
        while low + 1 < high {
            let count = (low + high) / 2;
            fill_fixed_page(&mut p, &knowledge, &evidence, count, signature)?;
            if fits(&p)? {
                low = count;
            } else {
                high = count;
            }
        }
        if low > 0 {
            fill_fixed_page(&mut p, &knowledge, &evidence, low, signature)?;
            return Ok(p);
        }
        p.saved_context.fixed_knowledge.clear();
        p.saved_context.fixed_evidence.clear();
        p.fixed_content_page.returned = 0;
        p.has_more = false;
        p.next_cursor = None;
    }
    if p.view != ResumeView::Compact && fits(&p)? {
        return Ok(p);
    }
    if !p.saved_context.snapshot.attempts.is_empty() {
        p.saved_context.snapshot.attempts.clear();
        omit(&mut p, ResumeSection::HistoricalAttempts);
    }
    if p.view != ResumeView::Compact && fits(&p)? {
        return Ok(p);
    }
    if !p.saved_context.snapshot.verifications.is_empty() {
        p.saved_context.snapshot.verifications.clear();
        omit(&mut p, ResumeSection::HistoricalVerifications);
    }
    if fits(&p)? {
        return Ok(p);
    }
    // Tiny metadata fields can cost less than their omission markers. Keep only reductions
    // that actually save bytes, so the budget error gives a usable minimum on retry.
    type Trim = fn(&mut ResumePackage);
    let trims: [(ResumeSection, Trim); 6] = [
        (ResumeSection::ReferencePurposes, |p| {
            let saved = &mut p.saved_context;
            for purpose in saved
                .knowledge_refs
                .iter_mut()
                .map(|r| &mut r.purpose)
                .chain(saved.evidence_refs.iter_mut().map(|r| &mut r.purpose))
            {
                if purpose.0.len() > 3 {
                    purpose.0 = "…".into();
                }
            }
        }),
        (ResumeSection::WorkspaceFileObservations, |p| {
            p.saved_context.snapshot.workspace.related_files.clear();
            p.saved_context
                .service_workspace_observation
                .related_files
                .clear();
            if let Observed::Known { value } = &mut p.current_workspace_observation {
                value.related_files.clear();
            }
        }),
        (ResumeSection::SnapshotNotes, |p| {
            p.saved_context.snapshot.no_references_reason = None;
        }),
        (ResumeSection::DerivedRequirements, |p| {
            p.resume_requirements.drain(..p.current_delta.len());
        }),
        (ResumeSection::FixedReferences, |p| {
            p.saved_context.knowledge_refs.clear();
            p.saved_context.evidence_refs.clear();
        }),
        (ResumeSection::SnapshotMetadata, |p| {
            let snapshot = &mut p.saved_context.snapshot;
            snapshot.completion_conditions.clear();
            if snapshot.scope.0.len() > 3 {
                snapshot.scope = Text("…".into());
            }
        }),
    ];
    let mut bytes = util::payload_bytes(&p)?;
    for (section, trim) in trims {
        let mut candidate = p.clone();
        trim(&mut candidate);
        omit(&mut candidate, section);
        let candidate_bytes = util::payload_bytes(&candidate)?;
        if candidate_bytes < bytes {
            p = candidate;
            bytes = candidate_bytes;
            if bytes <= budget as usize {
                break;
            }
        }
    }
    Ok(p)
}

fn fill_fixed_page(
    p: &mut ResumePackage,
    knowledge: &[FixedKnowledge],
    evidence: &[FixedEvidence],
    count: usize,
    signature: &str,
) -> Result<()> {
    p.saved_context.fixed_knowledge = knowledge[..count.min(knowledge.len())].to_vec();
    p.saved_context.fixed_evidence = evidence[..count.saturating_sub(knowledge.len())].to_vec();
    p.fixed_content_page.returned = count as u32;
    p.has_more = true;
    p.next_cursor = Some(util::cursor(&json!({
        "filter":signature,"checkpoint":p.saved_context.checkpoint_id,"offset":p.fixed_content_page.offset + count as u32
    }))?);
    Ok(())
}

fn omit(p: &mut ResumePackage, section: ResumeSection) {
    p.omitted_sections.push(section);
    let label = match section {
        ResumeSection::RelatedContext => "related_context",
        ResumeSection::FixedReferenceBodies => "fixed_reference_bodies",
        ResumeSection::HistoricalAttempts => "historical_attempts",
        ResumeSection::HistoricalVerifications => "historical_verifications",
        ResumeSection::ReferencePurposes => "reference_purposes",
        ResumeSection::FixedReferences => "fixed_references",
        ResumeSection::WorkspaceFileObservations => "workspace_file_observations",
        ResumeSection::SnapshotMetadata => "snapshot_metadata",
        ResumeSection::SnapshotNotes => "snapshot_notes",
        ResumeSection::DerivedRequirements => "derived_requirements",
    };
    p.omitted_categories.push(Text(label.into()));
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
fn availability(body: &Option<String>) -> RecordAvailability {
    if body.is_some() {
        RecordAvailability::Available
    } else {
        RecordAvailability::Purged
    }
}

fn memory_record(
    conn: &Connection,
    project: &str,
    id: &str,
    revision: Option<i64>,
) -> Result<MemoryRecord> {
    if let Some((r,claim,hash,status,meta))=conn.query_row("SELECT v.revision,v.claim,v.content_hash,v.status,v.metadata_json FROM knowledge_revisions v JOIN knowledge k ON v.project_id=k.project_id AND v.id=k.id WHERE v.project_id=?1 AND v.id=?2 AND v.revision=coalesce(?3,k.current_revision)",params![project,id,revision],|r|Ok((r.get(0)?,r.get::<_,Option<String>>(1)?,r.get(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional()? {
        return Ok(MemoryRecord::Found(FoundMemoryRecord::Knowledge {
            id: Identifier(id.into()), revision: Revision(r), availability: availability(&claim),
            claim: claim.map(Text), content_sha256: Sha256(hash),
            status: serde_json::from_value(Value::String(status))?, metadata: serde_json::from_str(&meta)?,
        }));
    }
    if let Some((source,observation))=conn.query_row("SELECT source_json,observation FROM evidence WHERE project_id=?1 AND id=?2 AND revision=coalesce(?3,1)",params![project,id,revision],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional()? {
        return Ok(MemoryRecord::Found(FoundMemoryRecord::Evidence {
            id: Identifier(id.into()), availability: availability(&observation),
            source: serde_json::from_str(&source)?, observation: observation.map(Text),
        }));
    }
    if let Some((raw,sequence,task_id,observed))=conn.query_row("SELECT snapshot_json,sequence,task_id,service_observation_json FROM checkpoints WHERE project_id=?1 AND id=?2 AND (?3 IS NULL OR sequence=?3)",params![project,id,revision],|r|Ok((r.get::<_,String>(0)?,r.get(1)?,r.get(2)?,r.get::<_,String>(3)?))).optional()? {
        return Ok(MemoryRecord::Found(FoundMemoryRecord::Checkpoint {
            id: Identifier(id.into()), sequence: Revision(sequence), task_id: Identifier(task_id),
            snapshot: serde_json::from_str(&raw)?, service_workspace_observation: serde_json::from_str(&observed)?,
            knowledge_refs: ref_rows(conn, "checkpoint_knowledge_refs", project, id)?,
            evidence_refs: ref_rows(conn, "checkpoint_evidence_refs", project, id)?,
        }));
    }
    Ok(MemoryRecord::Missing {
        id: Identifier(id.into()),
        availability: RecordAvailability::Missing,
    })
}
