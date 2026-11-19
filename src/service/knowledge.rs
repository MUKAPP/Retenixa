use super::*;
use std::collections::BTreeMap;
pub(super) fn validate_knowledge_ref(
    conn: &Connection,
    project: &str,
    r: &KnowledgeReference,
) -> Result<String> {
    let row=conn.query_row("SELECT v.claim,v.content_hash,k.status FROM knowledge_revisions v JOIN knowledge k ON v.project_id=k.project_id AND v.id=k.id WHERE v.project_id=?1 AND v.id=?2 AND v.revision=?3",params![project,r.knowledge_id.0,r.revision.0],|q|Ok((q.get::<_,Option<String>>(0)?,q.get::<_,String>(1)?,q.get::<_,String>(2)?))).optional()?;
    let Some((Some(_), hash, status)) = row else {
        return Err(DomainError::new(
            ErrorCode::ReferenceUnavailable,
            "固定知识版本缺失/清除或不属于当前项目",
        ));
    };
    if r.content_sha256.as_ref().is_some_and(|h| h.0 != hash) {
        return Err(DomainError::new(
            ErrorCode::ReferenceUnavailable,
            "固定知识内容哈希不一致",
        ));
    }
    Ok(status)
}
pub(super) fn validate_evidence_ref(
    conn: &Connection,
    project: &str,
    r: &EvidenceReference,
) -> Result<()> {
    let row=conn.query_row("SELECT source_json,content_hash,observation FROM evidence WHERE project_id=?1 AND id=?2 AND revision=?3",params![project,r.evidence_id.0,r.source_revision.0],|q|Ok((q.get::<_,String>(0)?,q.get::<_,String>(1)?,q.get::<_,Option<String>>(2)?))).optional()?;
    let Some((source, hash, Some(_))) = row else {
        return Err(DomainError::new(
            ErrorCode::ReferenceUnavailable,
            "固定证据版本缺失/清除或不属于当前项目",
        ));
    };
    let source: FixedEvidence = serde_json::from_str(&source)?;
    if hash != r.content_sha256.0 || source.observed_at.0 != r.observed_at.0 {
        return Err(DomainError::new(
            ErrorCode::ReferenceUnavailable,
            "固定证据哈希/观测时间不一致",
        ));
    }
    Ok(())
}
fn knowledge(conn: &Connection, project: &str, id: &str) -> Result<Value> {
    conn.query_row("SELECT k.key,k.scope_json,k.current_revision,k.status,v.claim,v.metadata_json FROM knowledge k JOIN knowledge_revisions v ON k.project_id=v.project_id AND k.id=v.id AND k.current_revision=v.revision WHERE k.project_id=?1 AND k.id=?2",params![project,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,String>(5)?))).optional()?.map(|(key,scope,rev,status,claim,metadata)|Ok(json!({"knowledge_id":id,"key":key,"scope":serde_json::from_str::<Value>(&scope)?,"revision":rev,"status":status,"claim":claim,"metadata":serde_json::from_str::<Value>(&metadata)?}))).unwrap_or_else(||Err(DomainError::new(ErrorCode::NotFound,"当前项目不存在该知识")))
}
fn scope(conn: &Connection, a: &Access, mut s: KnowledgeScope) -> Result<String> {
    if let Some(w) = &s.workspace_id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE project_id=?1 AND id=?2)",
            params![a.project, w.0],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(DomainError::new(
                ErrorCode::ProjectScopeMismatch,
                "知识 scope.workspace 不属于当前项目",
            ));
        }
    }
    if let Some(path) = &mut s.path_prefix {
        let replaced = path.0.replace('\\', "/");
        let p = std::path::Path::new(&replaced);
        if p.is_absolute()
            || p.components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(DomainError::invalid("scope.path_prefix 必须项目内相对路径"));
        }
        path.0 = p
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if path.0.is_empty() {
            return Err(DomainError::invalid("scope.path_prefix 不能为空路径"));
        }
    }
    s.conditions.sort_by(|a, b| a.0.cmp(&b.0));
    s.conditions.dedup_by(|a, b| a.0 == b.0);
    util::canonical(&util::json(&s)?)
}
fn sources(
    tx: &Transaction<'_>,
    a: &Access,
    refs: &[EvidenceReference],
    new: &[AgentEvidenceInput],
) -> Result<(Vec<EvidenceReference>, Vec<EntityVersion>)> {
    let mut all = BTreeMap::new();
    for r in refs {
        validate_evidence_ref(tx, &a.project, r)?;
        all.insert((r.evidence_id.0.clone(), r.source_revision.0), r.clone());
    }
    let mut keys = std::collections::HashSet::new();
    let mut versions = Vec::new();
    for n in new {
        if !keys.insert(&n.local_key.0) {
            return Err(DomainError::invalid("new_evidence.local_key 重复"));
        }
        let id = util::id();
        let origin = if let Some(origin) = &n.origin_observation_id {
            let known=tx.query_row("SELECT origin_observation_id FROM evidence WHERE project_id=?1 AND id=?2 AND revision=1",params![a.project,origin.0],|r|r.get::<_,String>(0)).optional()?;
            known.ok_or_else(|| {
                DomainError::new(
                    ErrorCode::ReferenceUnavailable,
                    "原始观测链必须在当前项目存在",
                )
            })?
        } else {
            id.clone()
        };
        let record = FixedEvidence {
            evidence_id: Identifier(id.clone()),
            source_revision: Revision(1),
            source: n.source.clone(),
            source_kind: SourceKind::AgentReported,
            content_sha256: n.content_sha256.clone(),
            observed_at: n.observed_at.clone(),
            origin_observation_id: Identifier(origin.clone()),
        };
        tx.execute(
            "INSERT INTO evidence VALUES(?1,?2,1,?3,?4,?5,?6)",
            params![
                a.project,
                id,
                serde_json::to_string(&record)?,
                n.observation.0,
                n.content_sha256.0,
                origin
            ],
        )?;
        all.insert(
            (id.clone(), 1),
            EvidenceReference {
                evidence_id: Identifier(id.clone()),
                source_revision: Revision(1),
                content_sha256: n.content_sha256.clone(),
                observed_at: n.observed_at.clone(),
                purpose: Text("知识版本提交时明确提供的来源".into()),
                selection: ReferenceSelection::CallerExplicit,
            },
        );
        versions.push(version(&id, 1));
    }
    Ok((all.into_values().collect(), versions))
}
impl Service {
    pub(super) fn write_knowledge(&self, input: KnowledgeWriteInput) -> Result<Value> {
        let payload = util::json(&input)?;
        let context = util::text(&payload, "context_id");
        let request = util::text(&payload, "request_id");
        let (_, replay) = self.preflight(&context, &request, "knowledge_write", &payload)?;
        if let Some(receipt) = replay {
            return Ok(receipt);
        }
        let (tokens, tags) = match &input {
            KnowledgeWriteInput::Propose {
                claim, search_tags, ..
            }
            | KnowledgeWriteInput::Revise {
                claim, search_tags, ..
            } => {
                if search_tags.as_ref().is_some_and(|tags| tags.len() > 8) {
                    return Err(DomainError::invalid("search_tags 最多 8 项"));
                }
                let tags = search_tags.as_ref().map(|tags| {
                    let mut tags = tags.iter().map(|tag| tag.0.trim()).collect::<Vec<_>>();
                    tags.sort_unstable();
                    tags.dedup();
                    json!(tags)
                });
                (Some(crate::search::tokenize(&claim.0)), tags)
            }
            _ => (None, None),
        };
        self.mutate(&context,&request,"knowledge_write",payload,|tx,a|{
        if let KnowledgeWriteInput::Propose{key,scope:given_scope,claim,evidence_refs,new_evidence,..}=&input {
            let key=key.0.trim().to_lowercase();let scope=scope(tx,a,given_scope.clone())?;let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM knowledge WHERE project_id=?1 AND key=?2 AND scope_json=?3)",params![a.project,key,scope],|r|r.get(0))?;
            if exists{return Err(DomainError::new(ErrorCode::VersionConflict,"expected=absent 未命中：同 key/scope 已有知识"));}
            let (refs,mut versions)=sources(tx,a,evidence_refs,new_evidence)?;
            let id=util::id();
            let mut meta=json!({"evidence_refs":refs,"source":"agent_reported","reported_by_workflow":a.workflow});
            if let Some(tags)=&tags {
                meta["search_tags"]=tags.clone();
            }
            tx.execute("INSERT INTO knowledge VALUES(?1,?2,?3,?4,1,'candidate')",params![a.project,id,key,scope])?;
            tx.execute("INSERT INTO knowledge_revisions VALUES(?1,?2,1,?3,?4,'candidate',?5,?6)",params![a.project,id,claim.0,util::hash(&claim.0),serde_json::to_string(&meta)?,util::now()])?;
            crate::search::insert(tx,&crate::search::knowledge_document(tx,&a.project,&id,tokens.clone())?)?;versions.push(version(&id,1));return Ok((json!({"knowledge":knowledge(tx,&a.project,&id)?,"evidence_refs":refs}),versions));
        }
        let (id,expected)=match &input {
            KnowledgeWriteInput::Accept{knowledge_id,expected_revision,..}|KnowledgeWriteInput::Revise{knowledge_id,expected_revision,..}|KnowledgeWriteInput::Dispute{knowledge_id,expected_revision,..}|KnowledgeWriteInput::Retract{knowledge_id,expected_revision,..}|KnowledgeWriteInput::Supersede{knowledge_id,expected_revision,..}=>(knowledge_id.0.clone(),expected_revision.0),
            KnowledgeWriteInput::Propose{..}=>unreachable!(),
        };
        let old=knowledge(tx,&a.project,&id)?;if old["revision"].as_i64()!=Some(expected){return Err(DomainError::new(ErrorCode::VersionConflict,"知识 revision 已改变"));}
        if expected>=9007199254740991{return Err(DomainError::invalid("知识版本计数已到上限"));}
        let mut claim=old["claim"].as_str().ok_or_else(||DomainError::new(ErrorCode::ReferenceUnavailable,"知识正文已清除"))?.to_owned();let mut metadata=old["metadata"].clone();let mut versions=Vec::new();
        let status=match &input {
            KnowledgeWriteInput::Accept{acceptance_basis,..}=>{
                if !matches!(old["status"].as_str(),Some("candidate"|"disputed")){return Err(DomainError::invalid("只有候选或争议知识可以显式接受"));}
                if acceptance_basis.evidence_refs.is_empty()&&acceptance_basis.agent_report.is_none(){return Err(DomainError::invalid("接受需要明确证据或可归因的 Agent 报告"));}
                for r in &acceptance_basis.evidence_refs{validate_evidence_ref(tx,&a.project,r)?;}
                metadata["acceptance_basis"]=util::json(acceptance_basis)?;metadata["accepted_by_workflow"]=json!(a.workflow);"active"
            },
            KnowledgeWriteInput::Revise{claim:new,evidence_refs,new_evidence,reason,..}=>{
                claim=new.0.clone();
                let (refs,new_versions)=sources(tx,a,evidence_refs,new_evidence)?;
                versions.extend(new_versions);
                let next_tags=tags.as_ref().or_else(||metadata.get("search_tags")).cloned();
                metadata=json!({"evidence_refs":refs,"reason":reason,"reported_by_workflow":a.workflow});
                if let Some(tags)=next_tags {
                    metadata["search_tags"]=tags;
                }
                "candidate"
            },
            KnowledgeWriteInput::Dispute{reason,..}=>{metadata["reason"]=util::json(reason)?;"disputed"},
            KnowledgeWriteInput::Retract{reason,..}=>{metadata["reason"]=util::json(reason)?;"retracted"},
            KnowledgeWriteInput::Supersede{replacement_id,replacement_expected_revision,reason,..}=>{
                if replacement_id.0==id{return Err(DomainError::invalid("知识不能替代自己"));}let replacement=knowledge(tx,&a.project,&replacement_id.0)?;
                if replacement["revision"].as_i64()!=Some(replacement_expected_revision.0){return Err(DomainError::new(ErrorCode::VersionConflict,"替代知识版本已变化"));}
                if replacement["status"]!="active"{return Err(DomainError::invalid("替代目标必须已明确接受为 active"));}metadata["reason"]=util::json(reason)?;metadata["replacement"]=replacement;"superseded"
            },
            _=>unreachable!(),
        };
        let rev=expected+1;tx.execute("INSERT INTO knowledge_revisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![a.project,id,rev,claim,util::hash(&claim),status,serde_json::to_string(&metadata)?,util::now()])?;
        let changed=tx.execute("UPDATE knowledge SET current_revision=?1,status=?2 WHERE project_id=?3 AND id=?4 AND current_revision=?5",params![rev,status,a.project,id,expected])?;
        if changed!=1{return Err(DomainError::new(ErrorCode::VersionConflict,"知识 CAS 未命中"));}
        crate::search::insert(tx,&crate::search::knowledge_document(tx,&a.project,&id,tokens.clone())?)?;
        versions.push(version(&id,rev));Ok((json!({"knowledge":knowledge(tx,&a.project,&id)?}),versions))
    })
    }
}
