use super::*;
use crate::search;
const FILTER: &str = r#"d.project_id=:project AND (:kind='all' OR d.kind=:kind)
 AND (:inactive=1 OR d.kind!='knowledge' OR d.status='active')
 AND (d.workspace_id IS NULL OR d.workspace_id=:workspace)
 AND (:task IS NULL OR d.task_id=:task OR (d.kind='knowledge' AND EXISTS(SELECT 1 FROM checkpoint_knowledge_refs r JOIN checkpoints c ON c.project_id=r.project_id AND c.id=r.checkpoint_id WHERE r.project_id=d.project_id AND r.object_id=d.entity_id AND c.task_id=:task)))
 AND (:path IS NULL OR NOT EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id) OR EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id AND (p.path=:path OR substr(p.path,1,length(:path)+1)=:path||'/' OR substr(:path,1,length(p.path)+1)=p.path||'/')))
 AND (:conditions='[]' OR NOT EXISTS(SELECT 1 FROM json_each(d.conditions_json)) OR EXISTS(SELECT 1 FROM json_each(:conditions) wanted JOIN json_each(d.conditions_json) actual ON actual.value=wanted.value))"#;
const COLUMNS: &str = "d.doc_id,d.kind,d.entity_id,d.revision,d.task_id,d.workspace_id,d.key,d.status,d.conditions_json,d.body_json,d.body_text,(SELECT json_group_array(p.path) FROM search_document_paths p WHERE p.row_id=d.row_id)";
const LITERAL_COLUMNS: &str = "d.doc_id,d.entity_id,d.key,d.body_text,d.conditions_json,json_extract(d.body_json,'$.metadata.search_tags'),(SELECT json_group_array(p.path) FROM search_document_paths p WHERE p.row_id=d.row_id)";
const LITERAL_WINDOW: usize = 500;
const LITERAL_SCAN_LIMIT: usize = 5_000;
const NEAR_PATH: &str = "(substr(p.path,1,length(:rank_path)+1)=:rank_path||'/' OR substr(:rank_path,1,length(p.path)+1)=p.path||'/' OR p.path=:query_dir OR substr(p.path,1,length(:query_dir)+1)=:query_dir||'/')";
const EXACT_MATCH: &str = "(d.doc_id=:query OR d.entity_id=:query OR coalesce(d.key=:query_lower,0) OR EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id AND p.path=:query_path))";
struct Hit {
    id: String,
    kind: String,
    entity: String,
    revision: i64,
    task: Option<String>,
    workspace: Option<String>,
    key: Option<String>,
    status: String,
    conditions: Value,
    body: Value,
    text: String,
    paths: Vec<String>,
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    Ok(Hit {
        id: r.get(0)?,
        kind: r.get(1)?,
        entity: r.get(2)?,
        revision: r.get(3)?,
        task: r.get(4)?,
        workspace: r.get(5)?,
        key: r.get(6)?,
        status: r.get(7)?,
        conditions: serde_json::from_str(&r.get::<_, String>(8)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        body: serde_json::from_str(&r.get::<_, String>(9)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?,
        text: r.get(10)?,
        paths: serde_json::from_str(&r.get::<_, String>(11)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}
fn prefix(path: &str) -> Result<String> {
    let p = path.replace('\\', "/");
    let p = std::path::Path::new(&p);
    if p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(DomainError::invalid("搜索 path_prefix 必须项目内相对路径"));
    }
    let value = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if value.is_empty() {
        return Err(DomainError::invalid("搜索路径不能为空"));
    }
    Ok(value)
}
fn literal_array_contains(json: &str, query: &str) -> Result<bool> {
    Ok(serde_json::from_str::<Vec<String>>(json)?
        .iter()
        .any(|value| value.to_lowercase().contains(query)))
}

fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let index = lower.find(&query.to_lowercase()).unwrap_or(0);
    let count = lower[..index].chars().count();
    text.chars()
        .skip(count.saturating_sub(40))
        .take(240)
        .collect()
}
fn render(h: &Hit, query: &str, method: &str, conditions: &[Text], path: Option<&str>) -> Value {
    let all_conditions = h.conditions.as_array().into_iter().flatten().all(|c| {
        conditions
            .iter()
            .any(|known| Some(known.0.as_str()) == c.as_str())
    });
    let path_matches = h.paths.is_empty()
        || path.is_some_and(|requested| {
            h.paths
                .iter()
                .any(|p| requested == p || requested.starts_with(&(p.clone() + "/")))
        });
    let applicability =
        if !all_conditions || !path_matches || (h.kind == "knowledge" && h.status != "active") {
            "needs_review"
        } else {
            "match"
        };
    let pointer = if h.kind == "knowledge" {
        json!({"id":h.entity,"revision":h.revision})
    } else {
        json!({"id":h.entity,"attempt_index":h.body["attempt_index"]})
    };
    let evidence = if h.kind == "knowledge" {
        h.body["metadata"]["evidence_refs"].clone()
    } else {
        h.body["attempt"]["verification_refs"].clone()
    };
    let selection = if h.kind == "knowledge" {
        json!({"knowledge_id":h.entity,"revision":h.revision,"content_sha256":util::hash(h.body["claim"].as_str().unwrap_or("")),"selection":"candidate_only","requires_explicit_adoption":true})
    } else {
        Value::Null
    };
    json!({"id":h.id,"kind":h.kind,"entity_id":h.entity,"revision":h.revision,"task_id":h.task,"workspace_id":h.workspace,"key":h.key,"status":h.status,"conditions":h.conditions,"paths":h.paths,"applicability":applicability,"snippet":snippet(&h.text,query),"evidence_refs":evidence,"full_record":pointer,"reference_candidate":selection,"search_method":method,"attempt":if h.kind=="attempts"{json!({"goal":snippet(h.body["attempt"]["goal"].as_str().unwrap_or(""),query),"method":snippet(h.body["attempt"]["method"].as_str().unwrap_or(""),query),"result":snippet(h.body["attempt"]["result"].as_str().unwrap_or(""),query),"reason":h.body["attempt"]["reason"],"state":h.body["attempt"]["state"]})}else{Value::Null}})
}
impl Service {
    pub(super) fn related_knowledge(
        &self,
        context: &str,
        a: &Access,
        snapshot: &CheckpointSnapshot,
        pins: &[KnowledgeReference],
        limit: usize,
    ) -> Result<(Vec<RelatedKnowledge>, bool)> {
        let mut paths = std::collections::BTreeSet::new();
        for path in snapshot
            .workspace
            .related_files
            .iter()
            .filter_map(|file| prefix(&file.path.0).ok())
        {
            paths.extend(path.match_indices('/').map(|(at, _)| path[..at].to_owned()));
            paths.insert(path);
        }
        let blockers = snapshot
            .blockers
            .iter()
            .take(4)
            .map(|s| s.0.chars().take(100).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ");
        let terms = search::fts_query(&blockers);
        if paths.is_empty() && pins.is_empty() && terms.is_none() {
            return Ok((Vec::new(), false));
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        self.access(&tx, context, false)?;
        let sql = format!(
            r#"
            WITH observed(path) AS MATERIALIZED (SELECT value FROM json_each(:paths)),
            pinned(id) AS MATERIALIZED (SELECT value FROM json_each(:pins)),
            fixed_scope(path) AS MATERIALIZED (
                SELECT DISTINCT p.path FROM pinned
                JOIN search_documents d ON d.project_id=:project AND d.doc_id=pinned.id
                JOIN search_document_paths p ON p.row_id=d.row_id
            ),
            observed_hits(row_id) AS MATERIALIZED (
                SELECT DISTINCT p.row_id FROM observed JOIN search_document_paths p ON p.path=observed.path
            ),
            fixed_hits(row_id) AS MATERIALIZED (
                SELECT DISTINCT p.row_id FROM fixed_scope JOIN search_document_paths p ON p.path=fixed_scope.path
            ),
            blocker_hits AS MATERIALIZED (
                SELECT rowid,{} AS score FROM search_fts
                WHERE :terms IS NOT NULL AND search_fts MATCH coalesce(:terms, '"retenixa_no_terms"')
            ),
            candidates(row_id) AS (
                SELECT row_id FROM observed_hits UNION SELECT row_id FROM fixed_hits UNION SELECT rowid FROM blocker_hits
            )
            SELECT d.entity_id,d.revision,d.key,substr(v.claim,1,100),
                CASE WHEN o.row_id IS NOT NULL THEN 0 WHEN f.row_id IS NOT NULL THEN 1 ELSE 2 END AS priority,
                d.conditions_json='[]'
                    AND (d.workspace_id IS NULL OR d.workspace_id=:current_workspace)
                    AND ((o.row_id IS NOT NULL AND :saved_workspace=:current_workspace)
                        OR NOT EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id))
                    AS applicable
            FROM candidates c JOIN search_documents d ON d.row_id=c.row_id
            JOIN knowledge k ON k.project_id=d.project_id AND k.id=d.entity_id AND k.current_revision=d.revision AND k.status='active'
            JOIN knowledge_revisions v ON v.project_id=k.project_id AND v.id=k.id AND v.revision=k.current_revision
            LEFT JOIN observed_hits o ON o.row_id=c.row_id
            LEFT JOIN fixed_hits f ON f.row_id=c.row_id
            LEFT JOIN blocker_hits b ON b.rowid=c.row_id
            WHERE d.project_id=:project AND d.kind='knowledge' AND v.claim IS NOT NULL
                AND (d.workspace_id IS NULL OR d.workspace_id=:saved_workspace OR d.workspace_id=:current_workspace)
                AND d.entity_id NOT IN (SELECT id FROM pinned)
            ORDER BY priority, applicable DESC, coalesce(b.score,0), d.entity_id LIMIT :limit
        "#,
            search::BM25
        );
        let mut hits = {
            let mut stmt = tx.prepare(&sql)?;
            stmt.query_map(rusqlite::named_params! {
                ":paths": serde_json::to_string(&paths)?,
                ":pins": serde_json::to_string(&pins.iter().map(|p| &p.knowledge_id).collect::<Vec<_>>())?,
                ":project": a.project,
                ":saved_workspace": snapshot.workspace.workspace_id.0,
                ":current_workspace": a.workspace,
                ":terms": terms,
                ":limit": limit as i64 + 1,
            }, |r| Ok(RelatedKnowledge {
                id: Identifier(r.get(0)?),
                revision: Revision(r.get(1)?),
                key: Identifier(r.get(2)?),
                excerpt: r.get(3)?,
                match_reason: Text(match r.get::<_, i64>(4)? {
                    0 => "适用路径覆盖检查点中的观测文件",
                    1 => "与固定知识的路径范围相同",
                    _ => "命中检查点记录的阻塞项",
                }.into()),
                applicability: if r.get::<_, bool>(5)? { Applicability::Match } else { Applicability::NeedsReview },
            }))?.collect::<std::result::Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        let more = hits.len() > limit;
        hits.truncate(limit);
        Ok((hits, more))
    }

    pub(super) fn memory_search(&self, mut input: MemorySearchInput) -> Result<Value> {
        if input.query.trim().is_empty()
            || input.query.chars().count() > 4096
            || !(1..=50).contains(&input.limit)
        {
            return Err(DomainError::invalid("query 需 1—4096 字符，limit 需 1—50"));
        }
        input.query = input.query.trim().into();
        let requested_path = input
            .scope
            .path_prefix
            .as_ref()
            .map(|p| prefix(&p.0))
            .transpose()?;
        input.scope.conditions.sort_by(|a, b| a.0.cmp(&b.0));
        input.scope.conditions.dedup_by(|a, b| a.0 == b.0);
        let terms = search::query_terms(&input.query);
        if terms.len() > 256 {
            return Err(DomainError::invalid("搜索分词超过 256 项，请缩短查询"));
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction()?;
        let a = self.access(&tx, &input.context_id.0, false)?;
        let workspace = input
            .scope
            .workspace_id
            .as_ref()
            .map(|w| w.0.as_str())
            .unwrap_or(&a.workspace);
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE project_id=?1 AND id=?2)",
            params![a.project, workspace],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(DomainError::new(
                ErrorCode::ProjectScopeMismatch,
                "检索 workspace 不属于当前项目",
            ));
        }
        if let Some(t) = &input.scope.task_id {
            task(&tx, &a.project, &t.0)?;
        }
        let kind = util::json(&input.kind)?;
        let kind = kind.as_str().unwrap();
        let conditions = serde_json::to_string(&input.scope.conditions)?;
        let fingerprint = util::hash(util::canonical(
            &json!({"project":a.project,"workspace":workspace,"query":input.query,"kind":kind,"scope":input.scope,"path":requested_path,"tokenizer":search::TOKENIZER_VERSION,"ranking":search::RANKING_VERSION}),
        )?);
        let query_lower = input.query.to_lowercase();
        let query_path = prefix(&input.query).ok();
        let query_dir = query_path
            .as_deref()
            .and_then(|p| p.rsplit_once('/').map(|(dir, _)| dir));
        let rank_path = requested_path.as_deref().or(query_path.as_deref());
        let mut mode = if input.query.chars().count() <= 2 || terms.is_empty() {
            "literal"
        } else {
            "fts"
        }
        .to_owned();
        let mut offset = 0i64;
        let mut after = String::new();
        if let Some(raw) = &input.cursor {
            let c = util::decode_cursor(raw)?;
            if c["filter"].as_str() != Some(&fingerprint) {
                return Err(DomainError::invalid("cursor 与项目/查询/排序不一致"));
            }
            mode = util::text(&c, "mode");
            offset = c["offset"]
                .as_i64()
                .filter(|offset| *offset >= 0)
                .ok_or_else(|| DomainError::invalid("cursor 位置必须为非负 64 位整数"))?;
            after = util::text(&c, "after");
            if !matches!(mode.as_str(), "literal" | "fts" | "fts_broad")
                || (mode == "fts_broad" && terms.len() < 2)
                || (mode != "literal" && terms.is_empty())
            {
                return Err(DomainError::invalid("cursor 模式或位置无效"));
            }
        }
        let page_cursor = |mode: &str, offset: i64, after: &str| {
            util::cursor(&json!({"filter":fingerprint,"mode":mode,"offset":offset,"after":after}))
        };
        let wanted = input.limit as usize + 1;
        let mut results = Vec::new();
        let mut next = None;
        let mut scanned = 0usize;
        let mut capped = false;
        let mut method = "literal";
        if mode != "literal" {
            let strict = terms.join(" AND ");
            let fts_page = |broad: bool, take: usize, skip: i64| -> Result<Vec<_>> {
                let stage = if broad { "fts_broad" } else { "fts" };
                let search_terms = if broad {
                    terms.join(" OR ")
                } else {
                    strict.clone()
                };
                let sql = format!(
                    r#"
                    WITH matched AS MATERIALIZED (
                        SELECT search_fts.rowid,{} AS score FROM search_fts
                        JOIN search_documents sd ON sd.row_id=search_fts.rowid
                        WHERE search_fts MATCH :match AND sd.project_id=:project
                    ), candidates(rowid) AS (
                        SELECT rowid FROM matched
                        UNION SELECT row_id FROM search_documents WHERE :broad=0 AND project_id=:project AND doc_id=:query
                        UNION SELECT row_id FROM search_documents WHERE :broad=0 AND project_id=:project AND entity_id=:query
                        UNION SELECT row_id FROM search_documents WHERE :broad=0 AND project_id=:project AND key=:query_lower
                        UNION SELECT row_id FROM search_document_paths WHERE :broad=0 AND path=:query_path
                    )
                    SELECT {COLUMNS} FROM candidates c JOIN search_documents d ON d.row_id=c.rowid
                        LEFT JOIN matched m ON m.rowid=c.rowid
                    WHERE {FILTER}
                        AND (:broad=0 OR (NOT {EXACT_MATCH} AND d.row_id NOT IN (
                            SELECT rowid FROM search_fts WHERE search_fts MATCH :strict)))
                    ORDER BY (d.doc_id=:query OR d.entity_id=:query OR coalesce(d.key=:query_lower,0)) DESC,
                        coalesce((SELECT max(CASE WHEN p.path=:query_path THEN 3
                             WHEN p.path=:rank_path THEN 2 WHEN {NEAR_PATH} THEN 1 ELSE 0 END)
                         FROM search_document_paths p WHERE p.row_id=d.row_id),0) DESC,
                        coalesce(m.score,0), d.doc_id LIMIT :limit OFFSET :offset
                "#,
                    search::BM25
                );
                let mut stmt = tx.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::named_params! {
                    ":project":a.project, ":kind":kind, ":inactive":input.scope.include_inactive,
                    ":workspace":workspace, ":task":input.scope.task_id.as_ref().map(|t|&t.0),
                    ":path":requested_path, ":conditions":conditions, ":match":search_terms,
                    ":query":input.query, ":query_lower":query_lower, ":query_path":query_path,
                    ":query_dir":query_dir, ":rank_path":rank_path, ":limit":take as i64,
                    ":offset":skip, ":broad":broad, ":strict":strict,
                }, row)?;
                rows.enumerate()
                    .map(|(i, hit)| {
                        let pos = skip.checked_add(i as i64 + 1).ok_or_else(|| {
                            DomainError::invalid("cursor 位置超出 SQLite OFFSET 范围")
                        })?;
                        Ok((hit?, stage, pos))
                    })
                    .collect()
            };
            let mut hits = fts_page(mode == "fts_broad", wanted, offset)?;
            // One extra hit establishes whether a continuation exists.
            if mode == "fts" && hits.len() < wanted && terms.len() > 1 {
                hits.extend(fts_page(true, wanted - hits.len(), 0)?);
            }
            if hits.is_empty() && input.cursor.is_none() {
                mode = "literal".into();
                method = "literal_fallback";
            } else {
                method = "fts_or_exact";
                for (i, (hit, stage, pos)) in hits.iter().take(input.limit as usize).enumerate() {
                    let candidate = hits
                        .get(i + 1)
                        .map(|(_, next_stage, _)| {
                            page_cursor(next_stage, if next_stage == stage { *pos } else { 0 }, "")
                        })
                        .transpose()?;
                    results.push(render(
                        hit,
                        &input.query,
                        method,
                        &input.scope.conditions,
                        requested_path.as_deref(),
                    ));
                    if let Err(e) = util::budget(
                        response(&results, candidate.as_deref(), method, 0, false),
                        input.byte_budget,
                        Some(hit.entity.clone()),
                    ) {
                        results.pop();
                        if results.is_empty() {
                            return Err(e);
                        }
                        break;
                    }
                    next = candidate;
                }
            }
        }
        if mode == "literal" {
            if input.cursor.is_some() && input.query.chars().count() > 2 && !terms.is_empty() {
                method = "literal_fallback";
            }
            let sql = format!(
                "SELECT {LITERAL_COLUMNS} FROM search_documents d WHERE {FILTER} AND d.doc_id>:after ORDER BY d.doc_id LIMIT :window"
            );
            let mut matches = Vec::new();
            let mut last_scanned = after;
            let mut stmt = tx.prepare(&sql)?;
            'windows: loop {
                let mut rows = stmt.query(rusqlite::named_params! {
                    ":project":a.project, ":kind":kind, ":inactive":input.scope.include_inactive,
                    ":workspace":workspace, ":task":input.scope.task_id.as_ref().map(|t|&t.0),
                    ":path":requested_path, ":conditions":conditions, ":after":last_scanned,
                    ":window":LITERAL_WINDOW as i64 + 1,
                })?;
                let mut window_scanned = 0;
                while let Some(candidate) = rows.next()? {
                    if scanned == LITERAL_SCAN_LIMIT {
                        capped = true;
                        break 'windows;
                    }
                    if window_scanned == LITERAL_WINDOW {
                        continue 'windows;
                    }
                    scanned += 1;
                    window_scanned += 1;
                    let id: String = candidate.get(0)?;
                    let entity: String = candidate.get(1)?;
                    let key: Option<String> = candidate.get(2)?;
                    let text: String = candidate.get(3)?;
                    let conditions_text: String = candidate.get(4)?;
                    let tags: Option<String> = candidate.get(5)?;
                    let paths: Vec<String> = serde_json::from_str(&candidate.get::<_, String>(6)?)?;
                    last_scanned.clone_from(&id);
                    if text.to_lowercase().contains(&query_lower)
                        || literal_array_contains(&conditions_text, &query_lower)?
                        || tags
                            .as_deref()
                            .map(|tags| literal_array_contains(tags, &query_lower))
                            .transpose()?
                            .unwrap_or(false)
                        || id == input.query
                        || entity == input.query
                        || key.as_deref() == Some(query_lower.as_str())
                        || paths
                            .iter()
                            .any(|p| p == &input.query || query_path.as_ref() == Some(p))
                    {
                        matches.push(id);
                        if matches.len() == wanted {
                            break 'windows;
                        }
                    }
                }
                break;
            }
            if matches.is_empty() && capped {
                next = Some(page_cursor("literal", 0, &last_scanned)?);
            }
            let full_sql = format!(
                "SELECT {COLUMNS} FROM search_documents d WHERE d.project_id=?1 AND d.doc_id=?2"
            );
            let mut full_stmt = tx.prepare(&full_sql)?;
            for (index, id) in matches.iter().take(input.limit as usize).enumerate() {
                let hit = full_stmt.query_row(params![a.project, id], row)?;
                results.push(render(
                    &hit,
                    &input.query,
                    method,
                    &input.scope.conditions,
                    requested_path.as_deref(),
                ));
                let candidate = if index + 1 < matches.len() {
                    Some(page_cursor("literal", 0, id)?)
                } else if capped {
                    Some(page_cursor("literal", 0, &last_scanned)?)
                } else {
                    None
                };
                if let Err(error) = util::budget(
                    response(&results, candidate.as_deref(), method, scanned, capped),
                    input.byte_budget,
                    Some(hit.entity.clone()),
                ) {
                    results.pop();
                    if results.is_empty() {
                        return Err(error);
                    }
                    break;
                }
                next = candidate;
            }
        }
        let output = util::budget(
            response(&results, next.as_deref(), method, scanned, capped),
            input.byte_budget,
            None,
        )?;
        tx.commit()?;
        Ok(output)
    }
}
fn response(
    results: &[Value],
    cursor: Option<&str>,
    method: &str,
    scanned: usize,
    capped: bool,
) -> Value {
    json!({"results":results,"has_more":cursor.is_some(),"next_cursor":cursor,"omitted_categories":[],"search_method":method,"literal_scan_count":scanned,"literal_scan_limit":LITERAL_SCAN_LIMIT,"scan_limit_reached":capped,"tokenizer_version":search::TOKENIZER_VERSION,"view":"live; concurrent revisions may change ordering; deduplicate by id+revision"})
}
