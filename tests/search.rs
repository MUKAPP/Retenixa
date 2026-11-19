use retenixa::{
    contracts::{DomainError, ErrorCode},
    search,
    service::Service,
    storage::{APPLICATION_ID, Store},
    util,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    service: Service,
    context: Value,
}
fn call(s: &Service, op: &str, v: Value) -> Value {
    s.call(op, v).unwrap_or_else(|e| panic!("{op}: {e}"))
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/search-tests")
            .join(util::id());
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&workspace)
                .args(["init", "-b", "synthetic"])
                .output()
                .unwrap()
                .status
                .success()
        );
        let service = Service::new(Store::open(root.join("data")).unwrap(), None);
        let context = call(
            &service,
            "context_open",
            json!({"workspace_path":workspace,"client_label":"search-tests","mode":"fresh","request_id":util::id()}),
        );
        Self {
            root,
            workspace,
            service,
            context,
        }
    }
    fn propose(&self, key: &str, claim: &str, scope: Value) -> Value {
        call(&self.service,"knowledge_write",json!({"op":"propose","context_id":self.context["context_id"],"request_id":util::id(),"expected":"absent","key":key,"claim":claim,"scope":scope,"evidence_refs":[],"new_evidence":[]}))["result"]["knowledge"].clone()
    }
    fn accept(&self, k: &Value) -> Value {
        call(&self.service,"knowledge_write",json!({"op":"accept","context_id":self.context["context_id"],"request_id":util::id(),"knowledge_id":k["knowledge_id"],"expected_revision":k["revision"],"acceptance_basis":{"rationale":"明确合成政策","evidence_refs":[],"agent_report":"由当前workflow提供的现场报告"}}))["result"]["knowledge"].clone()
    }
    fn search_input(&self, query: &str) -> Value {
        json!({"context_id":self.context["context_id"],"query":query,"kind":"all","scope":{"task_id":null,"workspace_id":null,"path_prefix":null,"include_inactive":false},"cursor":null,"limit":20,"byte_budget":65536})
    }
    fn search(&self, query: &str) -> Value {
        call(&self.service, "memory_search", self.search_input(query))
    }
    fn task(&self) -> Value {
        call(&self.service,"task_create",json!({"context_id":self.context["context_id"],"request_id":util::id(),"goal":"认证失败之后如何继续","scope":"搜索验收","assignment":"current_workflow","parent_task_id":null}))["result"]["task"].clone()
    }
    fn snapshot(&self) -> Value {
        json!({"schema_version":1,"goal":"认证失败之后如何继续","scope":"搜索验收","stage":"恢复","completion_conditions":["不丢未知操作"],"workspace":{"workspace_id":self.context["workspace_id"],"observed_at":util::now(),"branch":{"state":"unknown","reason":"服务采集"},"head":{"state":"unknown","reason":"空仓库"},"base_commit":{"state":"unknown","reason":"空仓库"},"related_files":[],"consistency":"unknown"},"progress":[],"attempts":[],"verifications":[],"unresolved_operations":[{"host":"synthetic","operation_id":"unknown-publish","description":"发布结果未知","state":"unknown","possible_side_effects":["可能已发布"],"verification_method":"查原操作"}],"blockers":[],"next_steps":["先核查原发布"],"no_references_reason":"暂无固定依据"})
    }
    fn save(&self, t: &Value, snapshot: Value, refs: Value) -> Value {
        call(&self.service,"checkpoint_save",json!({"context_id":self.context["context_id"],"request_id":util::id(),"task_id":t["task_id"],"expected_revision":t["revision"],"ownership_epoch":t["ownership_epoch"],"snapshot":snapshot,"knowledge_refs":refs,"evidence_refs":[],"target_status":"in_progress"}))["result"].clone()
    }
}
fn scope() -> Value {
    json!({"workspace_id":null,"path_prefix":null,"conditions":[]})
}
fn error(r: Result<Value, DomainError>, code: ErrorCode) {
    assert_eq!(format!("{:?}", r.unwrap_err().code), format!("{code:?}"));
}
#[test]
fn chinese_short_english_and_identifier_search_share_tokenizer() {
    let f = Fixture::new();
    let k=f.accept(&f.propose("auth","用户认证失败时调用refreshAccessToken，使用request_id重试。Reconnection memory retains original request.",scope()));
    for query in [
        "用户认证",
        "refreshAccessToken",
        "refresh",
        "access",
        "request_id",
        "request",
        "reconnection",
        "认证",
        "锁",
    ] {
        let page = f.search(query);
        if query != "锁" {
            assert!(
                page["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["entity_id"] == k["knowledge_id"]),
                "query={query} page={page}"
            );
        }
        assert_eq!(page["tokenizer_version"], search::TOKENIZER_VERSION);
    }
    let hit = &f.search("refresh")["results"][0];
    assert_eq!(hit["reference_candidate"]["selection"], "candidate_only");
    assert_eq!(
        hit["reference_candidate"]["requires_explicit_adoption"],
        true
    );
    assert_eq!(hit["full_record"]["revision"], 2);
}
#[test]
fn state_revision_and_fts_are_atomic() {
    let f = Fixture::new();
    let k = f.propose("candidate", "oldTokenOnly 原始结论", scope());
    assert!(
        f.search("oldTokenOnly")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let k = f.accept(&k);
    assert_eq!(f.search("oldTokenOnly")["results"][0]["revision"], 2);
    let updated=call(&f.service,"knowledge_write",json!({"op":"revise","context_id":f.context["context_id"],"request_id":util::id(),"knowledge_id":k["knowledge_id"],"expected_revision":2,"claim":"newUniqueMarker 修订结论","evidence_refs":[],"new_evidence":[],"reason":"现场改变"}))["result"]["knowledge"].clone();
    assert!(
        f.search("oldTokenOnly")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        f.search("newUniqueMarker")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut all = f.search_input("newUniqueMarker");
    all["scope"]["include_inactive"] = json!(true);
    assert_eq!(
        call(&f.service, "memory_search", all)["results"][0]["status"],
        "candidate"
    );
    let accepted = f.accept(&updated);
    call(
        &f.service,
        "knowledge_write",
        json!({"op":"retract","context_id":f.context["context_id"],"request_id":util::id(),"knowledge_id":accepted["knowledge_id"],"expected_revision":4,"reason":"撤回"}),
    );
    assert!(
        f.search("newUniqueMarker")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn paths_conditions_workspaces_and_projects_do_not_bleed() {
    let f = Fixture::new();
    let k=f.accept(&f.propose("scope","conditionalMarker",json!({"workspace_id":f.context["workspace_id"],"path_prefix":"src/auth","conditions":["Windows","offline"]})));
    let mut q = f.search_input("conditionalMarker");
    q["scope"]["path_prefix"] = json!("src/payment");
    assert!(
        call(&f.service, "memory_search", q.clone())["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    q["scope"]["path_prefix"] = json!("src/auth/file.rs");
    q["scope"]["conditions"] = json!(["Linux"]);
    assert!(
        call(&f.service, "memory_search", q.clone())["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    q["scope"]["conditions"] = json!(["offline", "Windows"]);
    assert_eq!(
        call(&f.service, "memory_search", q)["results"][0]["entity_id"],
        k["knowledge_id"]
    );
    let linked = f.root.join("linked");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&f.workspace)
            .args(["worktree", "add", "--orphan", "-b", "other"])
            .arg(&linked)
            .output()
            .unwrap()
            .status
            .success()
    );
    let other = Service::new(f.service.store.clone(), None);
    let c = call(
        &other,
        "context_open",
        json!({"workspace_path":linked,"client_label":"other","mode":"fresh","request_id":util::id()}),
    );
    let mut q = f.search_input("conditionalMarker");
    q["context_id"] = c["context_id"].clone();
    assert!(
        call(&other, "memory_search", q)["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let unrelated = Fixture::new();
    let third = Service::new(f.service.store.clone(), None);
    let c = call(
        &third,
        "context_open",
        json!({"workspace_path":unrelated.workspace,"client_label":"third","mode":"fresh","request_id":util::id()}),
    );
    let mut q = f.search_input("conditionalMarker");
    q["context_id"] = c["context_id"].clone();
    assert!(
        call(&third, "memory_search", q)["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn failed_attempts_return_conditions_method_reason_and_task_pointer() {
    let f = Fixture::new();
    let t = f.task();
    let mut snapshot = f.snapshot();
    snapshot["attempts"] = json!([{"goal":"认证连接","conditions":["offline"],"method":"refreshAccessToken","state":"failed","result":"连接失败","reason":{"state":"known","value":"本地端口关闭"},"verification_refs":[]},{"goal":"另一个方案","conditions":[],"method":"request_success","state":"verified","result":"成功","reason":{"state":"known","value":"服务在线"},"verification_refs":[]}]);
    let saved = f.save(&t, snapshot, json!([]));
    let mut q = f.search_input("refreshAccessToken");
    q["kind"] = json!("attempts");
    q["scope"]["task_id"] = t["task_id"].clone();
    q["scope"]["conditions"] = json!(["offline"]);
    let page = call(&f.service, "memory_search", q);
    let hit = &page["results"][0];
    assert_eq!(hit["conditions"], json!(["offline"]));
    assert_eq!(hit["attempt"]["reason"]["value"], "本地端口关闭");
    assert_eq!(hit["full_record"]["id"], saved["checkpoint_id"]);
    assert_eq!(hit["attempt"]["state"], "failed");
    assert!(
        f.search("request_success")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn text_query_is_not_fts_syntax_and_cursor_is_bound() {
    let f = Fixture::new();
    for i in 0..4 {
        f.accept(&f.propose(
            &format!("safe-{i}"),
            r#"sharedMarker quotes " OR NOT * % _"#,
            scope(),
        ));
    }
    for query in [r#"" OR NOT *"#, "%", "_", "NEAR(sharedMarker,5)"] {
        let _ = f.search(query);
    }
    let mut q = f.search_input("sharedMarker");
    q["limit"] = json!(1);
    let first = call(&f.service, "memory_search", q.clone());
    assert_eq!(first["has_more"], true);
    q["cursor"] = first["next_cursor"].clone();
    let second = call(&f.service, "memory_search", q.clone());
    assert_ne!(first["results"][0]["id"], second["results"][0]["id"]);
    q["query"] = json!("differentMarker");
    error(f.service.call("memory_search", q), ErrorCode::InvalidInput);
}
#[test]
fn bounded_literal_scan_cannot_claim_complete_absence() {
    let f = Fixture::new();
    for i in 0..503 {
        f.propose(&format!("scan-{i}"), "unrelated payload", scope());
    }
    let mut q = f.search_input("锚");
    q["scope"]["include_inactive"] = json!(true);
    let first = call(&f.service, "memory_search", q.clone());
    assert_eq!(first["scan_limit_reached"], true);
    assert_eq!(first["literal_scan_count"], 500);
    assert_eq!(first["has_more"], true);
    assert!(first["results"].as_array().unwrap().is_empty());
    q["cursor"] = first["next_cursor"].clone();
    let final_page = call(&f.service, "memory_search", q);
    assert_eq!(final_page["has_more"], false);
    assert_eq!(final_page["literal_scan_count"], 3);
}
#[test]
fn terminal_search_page_fits_its_actual_wire_budget() {
    let f = Fixture::new();
    f.accept(&f.propose("wire-boundary", "onlyMatchingMarker", scope()));
    for query in ["onlyMatchingMarker", "锚"] {
        if query == "锚" {
            f.accept(&f.propose("literal-boundary", "锚", scope()));
        }
        let full = f.search(query);
        assert_eq!(full["has_more"], false);
        let bytes = serde_json::to_vec(&rmcp::model::CallToolResult::structured(full.clone()))
            .unwrap()
            .len();
        let mut q = f.search_input(query);
        q["byte_budget"] = json!(bytes);
        let exact = call(&f.service, "memory_search", q);
        assert_eq!(exact["results"], full["results"]);
    }
}
#[test]
fn short_exact_path_and_partial_conditions_are_not_hidden_or_overstated() {
    let f = Fixture::new();
    let k = f.accept(&f.propose(
        "path-key",
        "unrelated content",
        json!({"workspace_id":null,"path_prefix":"锁","conditions":["windows","offline"]}),
    ));
    let mut q = f.search_input("锁");
    q["scope"]["conditions"] = json!(["offline"]);
    let partial = call(&f.service, "memory_search", q.clone());
    assert_eq!(partial["results"][0]["entity_id"], k["knowledge_id"]);
    assert_eq!(partial["results"][0]["applicability"], "needs_review");
    q["scope"]["conditions"] = json!(["offline", "windows"]);
    q["scope"]["path_prefix"] = json!("锁/file.rs");
    assert_eq!(
        call(&f.service, "memory_search", q)["results"][0]["applicability"],
        "match"
    );
}

#[test]
fn extra_known_conditions_keep_unconditional_and_subset_applicable_records() {
    let f = Fixture::new();
    let global = f.accept(&f.propose("unconditional", "conditionSubsetMarker", scope()));
    let subset = f.accept(&f.propose(
        "subset",
        "conditionSubsetMarker",
        json!({"workspace_id":null,"path_prefix":null,"conditions":["windows"]}),
    ));
    let mut q = f.search_input("conditionSubsetMarker");
    q["scope"]["conditions"] = json!(["windows", "offline"]);
    let page = call(&f.service, "memory_search", q);
    for k in [global, subset] {
        let hit = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["entity_id"] == k["knowledge_id"])
            .unwrap();
        assert_eq!(hit["applicability"], "match");
    }
}

#[test]
fn missing_tokenizer_version_blocks_access_until_rebuilt() {
    let f = Fixture::new();
    f.accept(&f.propose("rebuild", "恢复索引 rebuildMarker", scope()));
    f.service
        .store
        .connect()
        .unwrap()
        .execute(
            "UPDATE search_meta SET value='obsolete' WHERE key='tokenizer_version'",
            [],
        )
        .unwrap();
    error(
        f.service
            .call("memory_search", f.search_input("rebuildMarker")),
        ErrorCode::SchemaUnsupported,
    );
    search::rebuild(&f.service.store, false).unwrap();
    assert_eq!(
        f.search("rebuildMarker")["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn migration_preserves_authoritative_history_and_rebuilds_search() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/search-migration")
        .join(util::id());
    std::fs::create_dir_all(&root).unwrap();
    let conn = rusqlite::Connection::open(root.join("retenixa.sqlite3")).unwrap();
    conn.execute_batch(include_str!("../migrations/001_kernel.sql"))
        .unwrap();
    conn.pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute("INSERT INTO projects VALUES('p',?1)", [util::now()])
        .unwrap();
    let scope = serde_json::to_string(&scope()).unwrap();
    conn.execute(
        "INSERT INTO knowledge VALUES('p','k','legacy',?1,1,'active')",
        [scope],
    )
    .unwrap();
    conn.execute("INSERT INTO knowledge_revisions VALUES('p','k',1,'原始 migrationMarker',?1,'active','{}',?2)",rusqlite::params![util::hash("原始 migrationMarker"),util::now()]).unwrap();
    drop(conn);
    let store = Store::open(&root).unwrap();
    let c = store.connect().unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        retenixa::storage::SCHEMA_VERSION
    );
    assert_eq!(
        c.query_row(
            "SELECT claim FROM knowledge_revisions WHERE id='k'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "原始 migrationMarker"
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH ?1",
            [search::fts_query("migrationMarker").unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
#[test]
fn complete_single_pin_recovery_fits_exact_logical_budget() {
    let f = Fixture::new();
    let t = f.task();
    let k = f.accept(&f.propose("single-ref", "ZZZ-unrelated-proof", scope()));
    f.save(&t,f.snapshot(),json!([{"knowledge_id":k["knowledge_id"],"revision":2,"purpose":"pin","content_sha256":null,"selection":"caller_explicit"}]));
    let mut input = json!({"context_id":f.context["context_id"],"task_id":t["task_id"],"checkpoint_id":null,"cursor":null,"byte_budget":1048576});
    let first = call(&f.service, "context_resume", input.clone());
    assert_eq!(first["has_more"], false);
    let bytes = util::payload_bytes(&first).unwrap();
    input["byte_budget"] = json!(bytes);
    let final_page = call(&f.service, "context_resume", input);
    assert_eq!(
        final_page["saved_context"]["fixed_knowledge"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(final_page["next_cursor"], Value::Null);
}

#[test]
fn budgeted_resume_keeps_core_and_pages_fixed_bodies_without_autopin() {
    let f = Fixture::new();
    let t = f.task();
    let mut refs = Vec::new();
    for i in 0..5 {
        let k = f.accept(&f.propose(
            &format!("pin-{i}"),
            &format!("认证失败 {}", "old-body ".repeat(900)),
            scope(),
        ));
        refs.push(json!({"knowledge_id":k["knowledge_id"],"revision":2,"purpose":"明确使用的依据","content_sha256":null,"selection":"caller_explicit"}));
    }
    let saved = f.save(&t, f.snapshot(), json!(refs));
    let mut input = json!({"context_id":f.context["context_id"],"task_id":t["task_id"],"checkpoint_id":null,"cursor":null,"byte_budget":24000});
    let mut count = 0;
    let mut pages = 0;
    loop {
        let page = call(&f.service, "context_resume", input.clone());
        assert_eq!(
            page["saved_context"]["snapshot"]["unresolved_operations"][0]["operation_id"],
            "unknown-publish"
        );
        assert_eq!(
            page["saved_context"]["knowledge_refs"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            page["saved_context"]["checkpoint_id"],
            saved["checkpoint_id"]
        );
        count += page["saved_context"]["fixed_knowledge"]
            .as_array()
            .unwrap()
            .len();
        pages += 1;
        if page["has_more"] == false {
            break;
        }
        assert!(!page["omitted_categories"].as_array().unwrap().is_empty());
        input["cursor"] = page["next_cursor"].clone();
        assert!(pages < 10);
    }
    assert_eq!(count, 5);
    assert!(pages > 1);
    let db = f.service.store.connect().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM checkpoint_knowledge_refs", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        5
    );
}

#[test]
fn recovery_views_use_paths_pinned_scopes_and_blockers_without_including_claims() {
    let f = Fixture::new();
    let path_scope = |p| json!({"workspace_id":null,"path_prefix":p,"conditions":[]});
    let pin = f.accept(&f.propose("pin-origin", "fixed detail", path_scope("docs/frozen")));
    let observed = f.accept(&f.propose("file-policy", &"摘".repeat(130), path_scope("src/actual")));
    let same_scope = f.accept(&f.propose(
        "pinned-scope",
        "shared location",
        json!({"workspace_id":null,"path_prefix":"docs/frozen","conditions":["offline"]}),
    ));
    let blocked = f.accept(&f.propose("blocker-policy", "uniqueWorkflowBlocker", scope()));
    let mut snapshot = f.snapshot();
    snapshot["blockers"] = json!(["uniqueWorkflowBlocker"]);
    snapshot["workspace"]["related_files"] = json!([{"path":"src/actual/f.rs","observed_at":util::now(),"sha256":{"state":"unknown","reason":"服务观测"}}]);
    let task = f.task();
    let refs = json!([{"knowledge_id":pin["knowledge_id"],"revision":2,"purpose":"pin","content_sha256":null,"selection":"caller_explicit"}]);
    f.save(&task, snapshot, refs.clone());
    let mut input =
        json!({"context_id":f.context["context_id"],"task_id":task["task_id"],"byte_budget":64000});
    let standard = call(&f.service, "context_resume", input.clone());
    assert_eq!(standard["view"], "standard");
    assert_eq!(standard["related_context"].as_array().unwrap().len(), 2);
    assert_eq!(standard["omitted_sections"], json!(["related_context"]));
    let first = &standard["related_context"][0];
    assert_eq!(first["id"], observed["knowledge_id"]);
    assert_eq!(first["key"], "file-policy");
    assert_eq!(first["revision"], 2);
    assert_eq!(first["applicability"], "match");
    assert_eq!(first["excerpt"].as_str().unwrap().chars().count(), 100);
    assert!(first.get("claim").is_none());
    assert!(first["match_reason"].as_str().unwrap().contains("观测文件"));
    assert_eq!(
        standard["related_context"][1]["id"],
        same_scope["knowledge_id"]
    );
    assert_eq!(
        standard["related_context"][1]["applicability"],
        "needs_review"
    );
    input["view"] = json!("full");
    let full = call(&f.service, "context_resume", input.clone());
    assert_eq!(full["related_context"][2]["id"], blocked["knowledge_id"]);
    assert_eq!(
        full["saved_context"]["fixed_knowledge"][0]["knowledge_id"],
        pin["knowledge_id"]
    );
    input["view"] = json!("compact");
    let compact = call(&f.service, "context_resume", input);
    assert_eq!(compact["related_context"], json!([]));
    assert_eq!(compact["saved_context"]["fixed_knowledge"], json!([]));
    assert_eq!(compact["saved_context"]["knowledge_refs"], refs);
}

#[test]
fn recovery_budget_error_reports_an_exact_retry_size_and_core_survives() {
    let f = Fixture::new();
    let task = f.task();
    let mut snapshot = f.snapshot();
    snapshot["progress"] =
        json!([{"description":"CoreProgress".repeat(80),"state":"verified","artifacts":[]}]);
    snapshot["attempts"] = json!([{"goal":"optional history","conditions":[],"method":"trial","state":"failed","result":"large history".repeat(500),"reason":{"state":"known","value":"synthetic"},"verification_refs":[]}]);
    let saved = f.save(&task, snapshot.clone(), json!([]));
    let mut input =
        json!({"context_id":f.context["context_id"],"task_id":task["task_id"],"byte_budget":1});
    let error = util::json(&f.service.call("context_resume", input.clone()).unwrap_err()).unwrap();
    let min = error["min_required_bytes"].as_u64().unwrap();
    assert_eq!(error["required_bytes"], min);
    assert_eq!(error["full_result_pointer"], saved["checkpoint_id"]);
    assert_eq!(
        error["largest_fields"][0]["field"],
        "saved_context.snapshot.progress"
    );
    input["byte_budget"] = json!(min - 1);
    error_code_is_budget(&f, input.clone());
    input["byte_budget"] = json!(min);
    let page = call(&f.service, "context_resume", input);
    assert_eq!(util::payload_bytes(&page).unwrap() as u64, min);
    assert!(
        page["omitted_sections"]
            .as_array()
            .unwrap()
            .contains(&json!("historical_attempts"))
    );
    for key in ["goal", "progress", "unresolved_operations", "next_steps"] {
        assert_eq!(page["saved_context"]["snapshot"][key], snapshot[key]);
    }
}

fn error_code_is_budget(f: &Fixture, input: Value) {
    error(
        f.service.call("context_resume", input),
        ErrorCode::BudgetTooSmall,
    );
}
