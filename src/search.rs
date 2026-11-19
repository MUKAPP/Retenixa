use crate::{contracts::*, error::Result, util};
use jieba_rs::Jieba;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::LazyLock};
pub const TOKENIZER_VERSION: &str = "jieba-rs-0.11.0-default-search-hmm0-ident-fields-v2";
pub const RANKING_VERSION: &str = "strict-path-weights-v1";
/// FTS columns: body, key, conditions, tags.
pub const BM25: &str = "bm25(search_fts, 1.0, 8.0, 4.0, 2.0)";
static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);
pub fn tokenize(text: &str) -> String {
    let mut terms = BTreeSet::new();
    for token in JIEBA.cut_for_search(text, false) {
        let t = token.word.to_lowercase();
        if t.chars().any(char::is_alphanumeric) {
            terms.insert(t);
        }
    }
    // Jieba emits Han terms. Split at Han boundaries so adjacent identifiers still get segmented.
    for word in text.split(|c: char| {
        (!c.is_alphanumeric() && c != '_')
            || matches!(c, '\u{3400}'..='\u{9fff}' | '\u{20000}'..='\u{323af}')
    }) {
        if word.is_empty() {
            continue;
        }
        terms.insert(word.to_lowercase());
        let chars: Vec<char> = word.chars().collect();
        let mut part = String::new();
        for (i, c) in chars.iter().copied().enumerate() {
            let boundary = c == '_'
                || (i > 0
                    && c.is_uppercase()
                    && (chars[i - 1].is_lowercase()
                        || chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
            if boundary && !part.is_empty() {
                terms.insert(part.to_lowercase());
                part.clear();
            }
            if c != '_' {
                part.push(c);
            }
        }
        if !part.is_empty() {
            terms.insert(part.to_lowercase());
        }
    }
    terms
        .into_iter()
        .filter(|x| !x.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn is_stop_word(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "the"
            | "to"
            | "of"
            | "for"
            | "in"
            | "on"
            | "at"
            | "by"
            | "from"
            | "and"
            | "or"
            | "not"
            | "with"
            | "as"
            | "is"
            | "are"
            | "was"
            | "be"
            | "it"
            | "this"
            | "that"
            | "what"
            | "how"
            | "why"
            | "can"
            | "could"
            | "should"
            | "please"
            | "fn"
            | "let"
            | "mut"
            | "const"
            | "var"
            | "pub"
            | "use"
            | "impl"
            | "self"
            | "import"
            | "export"
            | "def"
            | "function"
            | "return"
            | "async"
            | "await"
            | "true"
            | "false"
            | "的"
            | "了"
            | "是"
            | "在"
            | "和"
            | "与"
            | "或"
            | "及"
            | "把"
            | "被"
            | "我"
            | "你"
            | "有"
            | "为"
            | "如何"
            | "怎么"
            | "为什么"
            | "是否"
            | "什么"
            | "请"
            | "这个"
            | "那个"
            | "一下"
            | "一个"
            | "可以"
            | "需要"
            | "进行"
            | "使用"
    )
}

fn quote_terms(tokens: &str, filter_stop_words: bool) -> Vec<String> {
    let terms: Vec<_> = tokens
        .split_whitespace()
        .filter(|s| s.chars().any(char::is_alphanumeric))
        .collect();
    // A stop word can be intentional by itself (for example, Rust's fn).
    let filter = filter_stop_words && terms.len() > 1;
    terms
        .into_iter()
        .filter(|term| !filter || !is_stop_word(term))
        .map(|term| format!("\"{}\"", term.replace('"', r#""""#)))
        .collect()
}

pub fn query_terms(query: &str) -> Vec<String> {
    quote_terms(&tokenize(query), true)
}

pub fn fts_query(query: &str) -> Option<String> {
    let tokens = tokenize(query);
    let mut terms = quote_terms(&tokens, true);
    if terms.is_empty() {
        // Blockers are saved symptoms; preserve recall when they contain only stop words.
        terms = quote_terms(&tokens, false);
    }
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

#[derive(Clone)]
pub struct Document {
    pub project: String,
    pub id: String,
    pub kind: String,
    pub entity: String,
    pub revision: i64,
    pub task: Option<String>,
    pub workspace: Option<String>,
    pub key: Option<String>,
    pub status: String,
    pub conditions: Value,
    pub body: Value,
    pub text: String,
    pub paths: Vec<String>,
    tokens: [String; 4],
}
fn terms_for_values(values: &Value) -> String {
    tokenize(
        &values
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

impl Document {
    fn set_tokens(&mut self, body_tokens: Option<String>) {
        let body = self.body["claim"].as_str().unwrap_or(&self.text);
        self.tokens = [
            body_tokens.unwrap_or_else(|| tokenize(body)),
            self.key.as_deref().map(tokenize).unwrap_or_default(),
            terms_for_values(&self.conditions),
            terms_for_values(&self.body["metadata"]["search_tags"]),
        ];
    }
}

pub fn knowledge_document(
    conn: &Connection,
    project: &str,
    id: &str,
    body_tokens: Option<String>,
) -> Result<Document> {
    let (key,scope,revision,status,claim,meta):(String,String,i64,String,String,String)=conn.query_row("SELECT k.key,k.scope_json,k.current_revision,k.status,v.claim,v.metadata_json FROM knowledge k JOIN knowledge_revisions v ON v.project_id=k.project_id AND v.id=k.id AND v.revision=k.current_revision WHERE k.project_id=?1 AND k.id=?2 AND v.claim IS NOT NULL",params![project,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let scope: Value = serde_json::from_str(&scope)?;
    let conditions = scope["conditions"].clone();
    let text = format!("{key} {claim}");
    let mut doc = Document {
        project: project.into(),
        id: id.into(),
        kind: "knowledge".into(),
        entity: id.into(),
        revision,
        task: None,
        workspace: scope["workspace_id"].as_str().map(str::to_owned),
        key: Some(key),
        status: status.clone(),
        conditions,
        body: json!({"knowledge_id":id,"claim":claim,"scope":scope,"status":status,"metadata":serde_json::from_str::<Value>(&meta)?}),
        text,
        paths: scope["path_prefix"]
            .as_str()
            .map(|p| vec![p.into()])
            .unwrap_or_default(),
        tokens: Default::default(),
    };
    if body_tokens.is_some() {
        doc.set_tokens(body_tokens);
    } else {
        doc.tokens = conn.query_row(
            "SELECT tokenized,key_tokens,condition_tokens,tag_tokens FROM search_documents WHERE project_id=?1 AND doc_id=?2",
            params![project, id],
            |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]),
        )?;
    }
    Ok(doc)
}
pub fn attempt_documents(
    project: &str,
    checkpoint: &str,
    task: &str,
    revision: i64,
    snapshot: &CheckpointSnapshot,
) -> Result<Vec<Document>> {
    let mut docs = unindexed_attempt_documents(project, checkpoint, task, revision, snapshot)?;
    for document in &mut docs {
        document.set_tokens(None);
    }
    Ok(docs)
}
fn unindexed_attempt_documents(
    project: &str,
    checkpoint: &str,
    task: &str,
    revision: i64,
    snapshot: &CheckpointSnapshot,
) -> Result<Vec<Document>> {
    snapshot
        .attempts
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.state, ExecutionState::Failed | ExecutionState::Unknown))
        .map(|(i, a)| {
            let body =
                json!({"attempt_index":i,"checkpoint_id":checkpoint,"task_id":task,"attempt":a});
            let reason = match &a.reason {
                Observed::Known { value } => &value.0,
                Observed::Unknown { reason } => &reason.0,
            };
            let text = format!("{} {} {} {}", a.goal.0, a.method.0, a.result.0, reason);
            Ok(Document {
                project: project.into(),
                id: format!("{checkpoint}:attempt:{i}"),
                kind: "attempts".into(),
                entity: checkpoint.into(),
                revision,
                task: Some(task.into()),
                workspace: Some(snapshot.workspace.workspace_id.0.clone()),
                key: None,
                status: if matches!(a.state, ExecutionState::Failed) {
                    "failed"
                } else {
                    "unknown"
                }
                .into(),
                conditions: util::json(&a.conditions)?,
                body,
                text,
                paths: snapshot
                    .workspace
                    .related_files
                    .iter()
                    .map(|p| p.path.0.replace('\\', "/"))
                    .collect(),
                tokens: Default::default(),
            })
        })
        .collect()
}
pub fn insert(conn: &Connection, d: &Document) -> Result<()> {
    conn.execute(
        "DELETE FROM search_documents WHERE project_id=?1 AND doc_id=?2",
        params![d.project, d.id],
    )?;
    conn.execute("INSERT INTO search_documents(project_id,doc_id,kind,entity_id,revision,task_id,workspace_id,key,status,conditions_json,body_json,body_text,tokenized,key_tokens,condition_tokens,tag_tokens) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![d.project,d.id,d.kind,d.entity,d.revision,d.task,d.workspace,d.key,d.status,serde_json::to_string(&d.conditions)?,serde_json::to_string(&d.body)?,d.text,d.tokens[0],d.tokens[1],d.tokens[2],d.tokens[3]])?;
    let row = conn.last_insert_rowid();
    for p in &d.paths {
        conn.execute(
            "INSERT OR IGNORE INTO search_document_paths VALUES(?1,?2)",
            params![row, p],
        )?;
    }
    Ok(())
}
fn source_manifest(
    conn: &Connection,
    mut visit: impl FnMut(Value) -> Result<()>,
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut stmt=conn.prepare("SELECT k.project_id,k.id,k.key,k.scope_json,k.current_revision,k.status,v.claim,v.metadata_json FROM knowledge k JOIN knowledge_revisions v ON v.project_id=k.project_id AND v.id=k.id AND v.revision=k.current_revision WHERE v.claim IS NOT NULL ORDER BY k.project_id,k.id")?;
    for row in stmt.query_map([], |r| {
        Ok(json!([
            "knowledge",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?
        ]))
    })? {
        let row = row?;
        digest.update(serde_json::to_vec(&row)?);
        digest.update(b"\n");
        visit(row)?;
    }
    let mut stmt=conn.prepare("SELECT project_id,id,task_id,task_revision,snapshot_json FROM checkpoints ORDER BY project_id,id")?;
    for row in stmt.query_map([], |r| {
        Ok(json!([
            "checkpoint",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?
        ]))
    })? {
        let row = row?;
        digest.update(serde_json::to_vec(&row)?);
        digest.update(b"\n");
        visit(row)?;
    }
    Ok(digest.finalize().into())
}
pub fn rebuild(root: &crate::storage::Store, force: bool) -> Result<()> {
    let mut conn = Connection::open(root.db_path())?;
    crate::storage::configure(&conn)?;
    let tx = conn.transaction()?;
    crate::storage::assert_schema(&tx)?;
    let old: Option<String> = tx
        .query_row(
            "SELECT value FROM search_meta WHERE key='tokenizer_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if !force && old.as_deref() == Some(TOKENIZER_VERSION) {
        return Ok(());
    }
    let mut docs = Vec::new();
    let manifest = source_manifest(&tx, |row| {
        if row[0] == "knowledge" {
            let project = row[1].as_str().unwrap();
            let id = row[2].as_str().unwrap();
            let scope: Value = serde_json::from_str(row[4].as_str().unwrap())?;
            let key = row[3].as_str().unwrap();
            let claim = row[7].as_str().unwrap();
            let text = format!("{key} {claim}");
            docs.push(Document{project:project.into(),id:id.into(),kind:"knowledge".into(),entity:id.into(),revision:row[5].as_i64().unwrap(),task:None,workspace:scope["workspace_id"].as_str().map(str::to_owned),key:Some(key.into()),status:row[6].as_str().unwrap().into(),conditions:scope["conditions"].clone(),body:json!({"knowledge_id":id,"claim":claim,"scope":scope,"status":row[6],"metadata":serde_json::from_str::<Value>(row[8].as_str().unwrap())?}),tokens:Default::default(),text,paths:scope["path_prefix"].as_str().map(|p|vec![p.into()]).unwrap_or_default()});
        } else {
            let snapshot: CheckpointSnapshot = serde_json::from_str(row[5].as_str().unwrap())?;
            docs.extend(unindexed_attempt_documents(
                row[1].as_str().unwrap(),
                row[2].as_str().unwrap(),
                row[3].as_str().unwrap(),
                row[4].as_i64().unwrap(),
                &snapshot,
            )?);
        }
        Ok(())
    })?;
    tx.commit()?;
    for document in &mut docs {
        document.set_tokens(None);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::storage::assert_schema(&tx)?;
    if source_manifest(&tx, |_| Ok(()))? != manifest {
        return Err(DomainError::storage(
            "索引重建期间权威版本变化；此次未替换索引，请稍后重新执行",
        ));
    }
    tx.execute("DELETE FROM search_documents", [])?;
    for d in &docs {
        insert(&tx, d)?;
    }
    tx.execute("INSERT INTO search_meta VALUES('tokenizer_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[TOKENIZER_VERSION])?;
    tx.commit()?;
    Ok(())
}
pub fn assert_index(conn: &Connection) -> Result<()> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM search_meta WHERE key='tokenizer_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if version.as_deref() != Some(TOKENIZER_VERSION) {
        return Err(DomainError::new(
            ErrorCode::SchemaUnsupported,
            "tokenizer 版本变化，需要完成派生索引重建后再写入",
        ));
    }
    Ok(())
}
