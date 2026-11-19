use super::*;
use crate::search;
const FILTER: &str = r#"d.project_id=:project AND (:kind='all' OR d.kind=:kind)
 AND (:inactive=1 OR d.kind!='knowledge' OR d.status='active')
 AND (d.workspace_id IS NULL OR d.workspace_id=:workspace)
 AND (:task IS NULL OR d.task_id=:task OR (d.kind='knowledge' AND EXISTS(SELECT 1 FROM checkpoint_knowledge_refs r JOIN checkpoints c ON c.project_id=r.project_id AND c.id=r.checkpoint_id WHERE r.project_id=d.project_id AND r.object_id=d.entity_id AND c.task_id=:task)))
 AND (:path IS NULL OR NOT EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id) OR EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id AND (p.path=:path OR substr(p.path,1,length(:path)+1)=:path||'/' OR substr(:path,1,length(p.path)+1)=p.path||'/')))
 AND (:conditions='[]' OR NOT EXISTS(SELECT 1 FROM json_each(d.conditions_json)) OR EXISTS(SELECT 1 FROM json_each(:conditions) wanted JOIN json_each(d.conditions_json) actual ON actual.value=wanted.value))"#;
const COLUMNS: &str = "d.doc_id,d.kind,d.entity_id,d.revision,d.task_id,d.workspace_id,d.key,d.status,d.conditions_json,d.body_json,d.body_text,(SELECT json_group_array(p.path) FROM search_document_paths p WHERE p.row_id=d.row_id)";
const LITERAL_COLUMNS: &str = "d.doc_id,d.entity_id,d.key,d.body_text,(SELECT json_group_array(p.path) FROM search_document_paths p WHERE p.row_id=d.row_id)";
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
        let query_tokens = search::fts_query(&input.query);
        if query_tokens
            .as_ref()
            .is_some_and(|s| s.split(" OR ").count() > 256)
        {
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
            &json!({"project":a.project,"workspace":workspace,"query":input.query,"kind":kind,"scope":input.scope,"path":requested_path,"tokenizer":search::TOKENIZER_VERSION}),
        )?);
        let mut mode = if input.query.chars().count() <= 2 || query_tokens.is_none() {
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
            if !matches!(mode.as_str(), "literal" | "fts") {
                return Err(DomainError::invalid("cursor 模式或位置无效"));
            }
        }
        let mut results = Vec::new();
        let mut next = None;
        let mut scanned = 0usize;
        let mut capped = false;
        let mut method = mode.clone();
        if mode == "fts" {
            let sql = format!(
                "WITH matched AS MATERIALIZED (SELECT rowid,bm25(search_fts) AS score FROM search_fts WHERE search_fts MATCH :match) SELECT {COLUMNS} FROM search_documents d LEFT JOIN matched m ON m.rowid=d.row_id WHERE {FILTER} AND (m.rowid IS NOT NULL OR d.doc_id=:query OR d.entity_id=:query OR d.key=lower(:query) OR EXISTS(SELECT 1 FROM search_document_paths p WHERE p.row_id=d.row_id AND p.path=:query)) ORDER BY CASE WHEN d.doc_id=:query OR d.entity_id=:query OR d.key=lower(:query) THEN 0 ELSE 1 END,coalesce(m.score,0),d.doc_id LIMIT :limit OFFSET :offset"
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows=stmt.query_map(rusqlite::named_params!{":project":a.project,":kind":kind,":inactive":input.scope.include_inactive,":workspace":workspace,":task":input.scope.task_id.as_ref().map(|t|&t.0),":path":requested_path,":conditions":conditions,":match":query_tokens.as_deref().unwrap_or("no_query_terms"),":query":input.query,":limit":input.limit+1,":offset":offset},row)?.collect::<std::result::Result<Vec<_>,_>>()?;
            if rows.is_empty() && input.cursor.is_none() {
                mode = "literal".into();
                method = "literal_fallback".into();
            } else {
                let mut used = 0;
                let mut position = offset;
                for hit in rows.iter().take(input.limit as usize) {
                    results.push(render(
                        hit,
                        &input.query,
                        "fts_or_exact",
                        &input.scope.conditions,
                        requested_path.as_deref(),
                    ));
                    let next_position = position.checked_add(1).ok_or_else(|| {
                        DomainError::invalid("cursor 位置超出 SQLite OFFSET 范围")
                    })?;
                    let c = util::cursor(
                        &json!({"filter":fingerprint,"mode":"fts","offset":next_position,"after":""}),
                    )?;
                    if let Err(e) = util::budget(
                        response(
                            &results,
                            if ((used + 1) as usize) < rows.len() {
                                Some(&c)
                            } else {
                                None
                            },
                            "fts_or_exact",
                            0,
                            false,
                        ),
                        input.byte_budget,
                        Some(hit.entity.clone()),
                    ) {
                        results.pop();
                        if results.is_empty() {
                            return Err(e);
                        }
                        break;
                    }
                    used += 1;
                    position = next_position;
                }
                if rows.len() > used as usize {
                    next = Some(util::cursor(
                        &json!({"filter":fingerprint,"mode":"fts","offset":position,"after":""}),
                    )?);
                }
                method = "fts_or_exact".into();
            }
        }
        if mode == "literal" {
            let sql = format!(
                "SELECT {LITERAL_COLUMNS} FROM search_documents d WHERE {FILTER} AND d.doc_id>:after ORDER BY d.doc_id LIMIT 501"
            );
            let query = input.query.to_lowercase();
            let query_path = prefix(&input.query).ok();
            let mut matches = Vec::new();
            let mut last_scanned = String::new();
            {
                let mut stmt = tx.prepare(&sql)?;
                let mut rows = stmt.query(rusqlite::named_params!{":project":a.project,":kind":kind,":inactive":input.scope.include_inactive,":workspace":workspace,":task":input.scope.task_id.as_ref().map(|t|&t.0),":path":requested_path,":conditions":conditions,":after":after})?;
                while let Some(candidate) = rows.next()? {
                    if scanned == 500 {
                        capped = true;
                        break;
                    }
                    scanned += 1;
                    let id: String = candidate.get(0)?;
                    let entity: String = candidate.get(1)?;
                    let key: Option<String> = candidate.get(2)?;
                    let text: String = candidate.get(3)?;
                    let paths: Vec<String> = serde_json::from_str(&candidate.get::<_, String>(4)?)?;
                    last_scanned.clone_from(&id);
                    if text.to_lowercase().contains(&query)
                        || id == input.query
                        || entity == input.query
                        || key.as_deref() == Some(query.as_str())
                        || paths
                            .iter()
                            .any(|p| p == &input.query || query_path.as_ref() == Some(p))
                    {
                        matches.push(id);
                    }
                }
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
                    &method,
                    &input.scope.conditions,
                    requested_path.as_deref(),
                ));
                let remaining = index + 1 < matches.len() || capped;
                let after = if index + 1 == matches.len() && capped {
                    last_scanned.as_str()
                } else {
                    hit.id.as_str()
                };
                let candidate = if remaining {
                    Some(util::cursor(
                        &json!({"filter":fingerprint,"mode":"literal","offset":0,"after":after}),
                    )?)
                } else {
                    None
                };
                if let Err(error) = util::budget(
                    response(&results, candidate.as_deref(), &method, scanned, capped),
                    input.byte_budget,
                    Some(hit.entity.clone()),
                ) {
                    results.pop();
                    if results.is_empty() {
                        return Err(error);
                    }
                    break;
                }
            }
            if results.len() < matches.len() {
                let after = &matches[results.len() - 1];
                next = Some(util::cursor(
                    &json!({"filter":fingerprint,"mode":"literal","offset":0,"after":after}),
                )?);
            } else if capped {
                next = Some(util::cursor(
                    &json!({"filter":fingerprint,"mode":"literal","offset":0,"after":last_scanned}),
                )?);
            }
        }
        let output = util::budget(
            response(&results, next.as_deref(), &method, scanned, capped),
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
    json!({"results":results,"has_more":cursor.is_some(),"next_cursor":cursor,"omitted_categories":[],"search_method":method,"literal_scan_count":scanned,"literal_scan_limit":500,"scan_limit_reached":capped,"tokenizer_version":search::TOKENIZER_VERSION,"view":"live; concurrent revisions may change ordering; deduplicate by id+revision"})
}
