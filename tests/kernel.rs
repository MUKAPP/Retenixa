use retenixa::{
    contracts::{DomainError, ErrorCode},
    maintenance,
    service::{HostAttachment, Service},
    storage::Store,
    util,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
};
struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    data: PathBuf,
}
fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/kernel-tests")
            .join(util::id());
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        git(&workspace, &["init", "-b", "test-main"]);
        let data = root.join("data");
        Self {
            root,
            workspace,
            data,
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.data).unwrap()
    }
    fn fresh(&self) -> (Service, Value) {
        let service = Service::new(self.store(), None);
        let context = open(&service, &self.workspace, "fresh");
        (service, context)
    }
    fn binding(&self) -> HostAttachment {
        let path = self.root.join("private/binding.json");
        HostAttachment::ensure(&path, "dsh", "synthetic-session").unwrap();
        HostAttachment::load(&path).unwrap()
    }
}
fn call(service: &Service, op: &str, args: Value) -> Value {
    service
        .call(op, args)
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}
fn open(service: &Service, root: &Path, mode: &str) -> Value {
    call(
        service,
        "context_open",
        json!({"workspace_path":root,"client_label":"integration","mode":mode,"request_id":util::id()}),
    )
}
fn create(service: &Service, context: &Value, assignment: &str) -> Value {
    call(service,"task_create",json!({"context_id":context["context_id"],"request_id":util::id(),"goal":"保存并可靠接续","scope":"合成任务","assignment":assignment,"parent_task_id":null}))["result"]["task"].clone()
}
fn snapshot(context: &Value) -> Value {
    json!({"schema_version":1,"goal":"保存并可靠接续","scope":"合成范围","stage":"实现中","completion_conditions":["旧写者拒写"],"workspace":{"workspace_id":context["workspace_id"],"observed_at":util::now(),"branch":{"state":"unknown","reason":"由服务补采"},"base_commit":{"state":"unknown","reason":"空仓库"},"head":{"state":"unknown","reason":"空仓库"},"related_files":[],"consistency":"unknown"},"progress":[{"description":"阶段保存","state":"verified","artifacts":[]}],"attempts":[],"verifications":[],"unresolved_operations":[{"host":"synthetic-host","operation_id":"unknown-publish","description":"发布结果未知","state":"unknown","possible_side_effects":["可能已发布"],"verification_method":"先查询原operation id"}],"blockers":[],"next_steps":["先确认未知结果"],"no_references_reason":"尚无固定知识依据"})
}
fn save_input(context: &Value, task: &Value) -> Value {
    json!({"context_id":context["context_id"],"request_id":util::id(),"task_id":task["task_id"],"expected_revision":task["revision"],"ownership_epoch":task["ownership_epoch"],"snapshot":snapshot(context),"knowledge_refs":[],"evidence_refs":[],"target_status":"in_progress"})
}
fn error(result: Result<Value, DomainError>, code: ErrorCode) {
    let e = result.unwrap_err();
    assert_eq!(format!("{:?}", e.code), format!("{code:?}"), "{e}");
}
fn get_resume(service: &Service, context: &Value, task: &Value) -> Value {
    call(
        service,
        "context_resume",
        json!({"context_id":context["context_id"],"task_id":task["task_id"],"checkpoint_id":null,"cursor":null,"byte_budget":1048576}),
    )
}
fn propose(service: &Service, context: &Value, claim: &str) -> Value {
    call(service,"knowledge_write",json!({"op":"propose","context_id":context["context_id"],"request_id":util::id(),"expected":"absent","key":util::id(),"scope":{"workspace_id":null,"path_prefix":null,"conditions":[]},"claim":claim,"evidence_refs":[],"new_evidence":[]}))["result"]["knowledge"].clone()
}
#[test]
fn wal_schema_and_no_cross_project_reads() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let db = s.store.connect().unwrap();
    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    assert_eq!(
        db.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let t = create(&s, &c, "current_workflow");
    let other = Fixture::new();
    let shared = Service::new(s.store.clone(), None);
    let c2 = open(&shared, &other.workspace, "fresh");
    assert_ne!(c["project_id"], c2["project_id"]);
    let records = call(
        &shared,
        "memory_get",
        json!({"context_id":c2["context_id"],"ids":[t["task_id"]],"revision":null,"cursor":null,"byte_budget":8192}),
    );
    assert_eq!(records["records"][0]["availability"], "missing");
    let list = call(
        &shared,
        "task_list",
        json!({"context_id":c2["context_id"],"status":null,"workspace_id":null,"parent_task_id":null,"cursor":null,"limit":10}),
    );
    assert!(list["tasks"].as_array().unwrap().is_empty());
}
#[test]
fn linked_worktree_shares_project_but_has_own_workspace() {
    let f = Fixture::new();
    let (a, ca) = f.fresh();
    let linked = f.root.join("linked");
    git(
        &f.workspace,
        &[
            "worktree",
            "add",
            "--orphan",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ],
    );
    let b = Service::new(a.store.clone(), None);
    let cb = open(&b, &linked, "fresh");
    assert_eq!(ca["project_id"], cb["project_id"]);
    assert_ne!(ca["workspace_id"], cb["workspace_id"]);
}
#[test]
fn duplicate_receipt_and_changed_payload() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let input = json!({"context_id":c["context_id"],"request_id":"same-operation","goal":"原目标","scope":"scope","assignment":"current_workflow","parent_task_id":null});
    let first = call(&s, "task_create", input.clone());
    let replay = call(&s, "task_create", input.clone());
    assert_eq!(first["result"], replay["result"]);
    assert_eq!(replay["replayed"], true);
    let mut changed = input;
    changed["goal"] = json!("不同目标");
    error(s.call("task_create", changed), ErrorCode::RequestIdReused);
    assert_eq!(
        s.store
            .connect()
            .unwrap()
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn concurrent_claim_is_atomic() {
    let f = Fixture::new();
    let (a, ca) = f.fresh();
    let task = create(&a, &ca, "unassigned");
    let b = Service::new(a.store.clone(), None);
    let cb = open(&b, &f.workspace, "fresh");
    let barrier = Arc::new(Barrier::new(2));
    let handlers=[(a,ca),(b,cb)].into_iter().map(|(s,c)|{let barrier=barrier.clone();let id=task["task_id"].clone();std::thread::spawn(move||{barrier.wait();s.call("task_claim",json!({"context_id":c["context_id"],"request_id":util::id(),"task_id":id,"expected_revision":1,"expected_epoch":1}))})}).collect::<Vec<_>>();
    let results = handlers
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
}
#[test]
fn response_loss_attachment_replay_only_confirms_then_new_cas_fences() {
    let f = Fixture::new();
    let host = f.binding();
    let a = Service::new(f.store(), Some(host.clone()));
    let ca = open(&a, &f.workspace, "continue");
    let t = create(&a, &ca, "current_workflow");
    let b = Service::new(a.store.clone(), Some(host.clone()));
    let confirmed = open(&b, &f.workspace, "continue");
    assert_eq!(confirmed["writer_generation"], 1);
    assert_eq!(confirmed["requires_reattach"], true);
    error(
        b.call("checkpoint_save", save_input(&confirmed, &t)),
        ErrorCode::StaleAttachment,
    );
    let still_a = open(&a, &f.workspace, "continue");
    assert_eq!(still_a["requires_reattach"], false);
    let mut acknowledged = host;
    acknowledged.expected_generation = 1;
    acknowledged.attachment_request_id = util::id();
    let current = Service::new(a.store.clone(), Some(acknowledged));
    let cc = open(&current, &f.workspace, "continue");
    assert_eq!(cc["workflow_id"], ca["workflow_id"]);
    assert_eq!(cc["writer_generation"], 2);
    error(
        a.call("checkpoint_save", save_input(&ca, &t)),
        ErrorCode::StaleAttachment,
    );
    for old in [&a, &b] {
        error(old.call("context_open",json!({"workspace_path":f.workspace,"client_label":"integration","mode":"continue","request_id":util::id()})),ErrorCode::StaleAttachment);
    }
    let saved = call(&current, "checkpoint_save", save_input(&cc, &t));
    assert_eq!(saved["result"]["task"]["revision"], 2);
}

#[test]
fn lost_capability_does_not_create_duplicate_trusted_host_identity() {
    let f = Fixture::new();
    let host = f.binding();
    let a = Service::new(f.store(), Some(host.clone()));
    open(&a, &f.workspace, "continue");
    let mut lost = host;
    lost.capability = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    lost.attachment_request_id = util::id();
    lost.expected_generation = 0;
    let b = Service::new(a.store.clone(), Some(lost));
    error(b.call("context_open",json!({"workspace_path":f.workspace,"client_label":"integration","mode":"continue","request_id":util::id()})),ErrorCode::ClaimDenied);
    assert_eq!(
        a.store
            .connect()
            .unwrap()
            .query_row("SELECT count(*) FROM workflows", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn trusted_host_identity_tuple_has_no_delimiter_collision() {
    let f = Fixture::new();
    let mut h = f.binding();
    h.host = "tool:local".into();
    h.host_session = "s".into();
    let a = Service::new(f.store(), Some(h.clone()));
    let ca = open(&a, &f.workspace, "continue");
    h.host = "tool".into();
    h.host_session = "local:s".into();
    h.attachment_request_id = util::id();
    let b = Service::new(a.store.clone(), Some(h));
    let cb = open(&b, &f.workspace, "continue");
    assert_ne!(ca["workflow_id"], cb["workflow_id"]);
}

#[test]
fn trusted_binding_hash_tuple_has_no_nul_delimiter_collision() {
    let f = Fixture::new();
    let mut h = f.binding();
    h.host = "tool\0local".into();
    h.host_session = "s".into();
    let a = Service::new(f.store(), Some(h.clone()));
    let ca = open(&a, &f.workspace, "continue");
    h.host = "tool".into();
    h.host_session = "local\0s".into();
    h.attachment_request_id = util::id();
    let b = Service::new(a.store.clone(), Some(h));
    let cb = open(&b, &f.workspace, "continue");
    assert_ne!(ca["workflow_id"], cb["workflow_id"]);
}

#[test]
fn ordinary_client_cannot_continue_by_name() {
    let f = Fixture::new();
    let s = Service::new(f.store(), None);
    error(s.call("context_open",json!({"workspace_path":f.workspace,"client_label":"integration","mode":"continue","request_id":util::id()})),ErrorCode::ClaimDenied);
}
#[test]
fn transfer_rejects_old_owner_but_allows_original_receipt_confirmation() {
    let f = Fixture::new();
    let (a, ca) = f.fresh();
    let t = create(&a, &ca, "current_workflow");
    let input = save_input(&ca, &t);
    let saved = call(&a, "checkpoint_save", input.clone());
    let b = Service::new(a.store.clone(), None);
    let cb = open(&b, &f.workspace, "fresh");
    let management = json!({"project_id":ca["project_id"],"task_id":t["task_id"],"target_workflow_id":cb["workflow_id"],"expected_revision":2,"expected_epoch":1,"request_id":util::id(),"reason":"合成管理确认"});
    error(
        maintenance::transfer(&a.store, util::parse(management.clone()).unwrap(), false),
        ErrorCode::ClaimDenied,
    );
    let transferred =
        maintenance::transfer(&a.store, util::parse(management).unwrap(), true).unwrap();
    let mut old = save_input(&ca, &saved["result"]["task"]);
    old["expected_revision"] = transferred["result"]["revision"].clone();
    old["ownership_epoch"] = transferred["result"]["ownership_epoch"].clone();
    error(a.call("checkpoint_save", old), ErrorCode::OwnershipChanged);
    let confirmed = call(&a, "checkpoint_save", input);
    assert_eq!(confirmed["replayed"], true);
    assert_eq!(confirmed["result"], saved["result"]);
    let next = call(
        &b,
        "checkpoint_save",
        save_input(&cb, &transferred["result"]),
    );
    assert_eq!(
        next["result"]["task"]["owner_workflow_id"],
        cb["workflow_id"]
    );
}
#[test]
fn invalid_pin_rolls_back_every_associated_write() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let mut input = save_input(&c, &t);
    input["knowledge_refs"] = json!([{"knowledge_id":"not-present","revision":1,"purpose":"missing","content_sha256":null,"selection":"caller_explicit"}]);
    let request = input["request_id"].clone();
    error(
        s.call("checkpoint_save", input),
        ErrorCode::ReferenceUnavailable,
    );
    let db = s.store.connect().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM checkpoints", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT revision FROM tasks WHERE id=?1",
            [t["task_id"].as_str().unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let result = call(
        &s,
        "request_get",
        json!({"context_id":c["context_id"],"request_id":request}),
    );
    assert_eq!(result["status"], "not_found");
}
#[test]
fn fixed_history_and_current_retraction_are_distinct() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let k = propose(&s, &c, "原始依据v1");
    let k=call(&s,"knowledge_write",json!({"op":"accept","context_id":c["context_id"],"request_id":util::id(),"knowledge_id":k["knowledge_id"],"expected_revision":1,"acceptance_basis":{"rationale":"合成接受政策","evidence_refs":[],"agent_report":"明确归因的Agent现场报告"}}))["result"]["knowledge"].clone();
    let mut input = save_input(&c, &t);
    input["knowledge_refs"] = json!([{"knowledge_id":k["knowledge_id"],"revision":2,"purpose":"原始判断依据","content_sha256":null,"selection":"caller_explicit"}]);
    let saved = call(&s, "checkpoint_save", input);
    call(
        &s,
        "knowledge_write",
        json!({"op":"retract","context_id":c["context_id"],"request_id":util::id(),"knowledge_id":k["knowledge_id"],"expected_revision":2,"reason":"新观测撤回"}),
    );
    let package = get_resume(&s, &c, &saved["result"]["task"]);
    assert_eq!(
        package["saved_context"]["fixed_knowledge"][0]["claim"],
        "原始依据v1"
    );
    assert_eq!(
        package["saved_context"]["fixed_knowledge"][0]["revision"],
        2
    );
    assert!(
        package["current_delta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["kind"] == "retracted")
    );
    assert!(
        package["resume_requirements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["subject_id"] == "unknown-publish")
    );
    let db = s.store.connect().unwrap();
    assert!(
        db.execute("UPDATE checkpoints SET snapshot_json='{}'", [])
            .is_err()
    );
    assert!(db.execute("DELETE FROM knowledge_revisions", []).is_err());
}
#[test]
fn explicit_evidence_is_fixed_and_cannot_spoof_source_level() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let proposed = call(
        &s,
        "knowledge_write",
        json!({"op":"propose","context_id":c["context_id"],"request_id":util::id(),"expected":"absent","key":"evidence-key","scope":{"workspace_id":null,"path_prefix":null,"conditions":[]},"claim":"source claim","evidence_refs":[],"new_evidence":[{"local_key":"one","source":"synthetic.txt","observation":"现场文字","content_sha256":util::hash("source"),"observed_at":util::now(),"origin_observation_id":null}]}),
    );
    let mut input = save_input(&c, &t);
    input["evidence_refs"] = proposed["result"]["evidence_refs"].clone();
    call(&s, "checkpoint_save", input);
    let package = get_resume(&s, &c, &t);
    assert_eq!(
        package["saved_context"]["fixed_evidence"][0]["source_kind"],
        "agent_reported"
    );
    let db = s.store.connect().unwrap();
    assert!(
        db.execute("UPDATE evidence SET observation='overwrite'", [])
            .is_err()
    );
}
#[test]
fn file_changes_budget_and_terminal_state() {
    let f = Fixture::new();
    std::fs::write(f.workspace.join("observed.txt"), "before").unwrap();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let mut input = save_input(&c, &t);
    input["snapshot"]["workspace"]["related_files"] = json!([{"path":"observed.txt","sha256":{"state":"unknown","reason":"服务采集"},"observed_at":util::now()}]);
    input["target_status"] = json!("completed");
    let saved = call(&s, "checkpoint_save", input);
    std::fs::write(f.workspace.join("observed.txt"), "after").unwrap();
    let package = get_resume(&s, &c, &t);
    assert!(
        package["current_delta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["reason"].as_str().unwrap().contains("observed.txt"))
    );
    let e=s.call("context_resume",json!({"context_id":c["context_id"],"task_id":t["task_id"],"checkpoint_id":null,"cursor":null,"byte_budget":1})).unwrap_err();
    assert!(matches!(e.code, ErrorCode::BudgetTooSmall));
    assert!(e.required_bytes.unwrap() > 1);
    assert!(e.full_result_pointer.is_some());
    error(
        s.call("checkpoint_save", save_input(&c, &saved["result"]["task"])),
        ErrorCode::InvalidInput,
    );
}
#[test]
fn backup_restore_requires_exclusive_maintenance_and_preserves_history() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let saved = call(&s, "checkpoint_save", save_input(&c, &t));
    let backup = f.root.join("backup.sqlite3");
    maintenance::backup(&s.store, &backup).unwrap();
    error(
        maintenance::restore(&f.data, &backup),
        ErrorCode::StorageBusy,
    );
    let before = maintenance::doctor(&f.data).unwrap();
    assert_eq!(before["integrity"], "ok");
    drop(s);
    let restored = maintenance::restore(&f.data, &backup).unwrap();
    assert_eq!(restored["integrity"], "ok");
    let reopened = f.store();
    assert_eq!(
        reopened
            .connect()
            .unwrap()
            .query_row("SELECT id FROM checkpoints", [], |r| r.get::<_, String>(0))
            .unwrap(),
        saved["result"]["checkpoint_id"]
    );
}
#[test]
fn checkpoint_receipt_replays_after_same_workflow_changes_worktree() {
    let f = Fixture::new();
    let host = f.binding();
    let a = Service::new(f.store(), Some(host.clone()));
    let ca = open(&a, &f.workspace, "continue");
    let t = create(&a, &ca, "current_workflow");
    let input = save_input(&ca, &t);
    let committed = call(&a, "checkpoint_save", input.clone());
    let linked = f.root.join("linked");
    git(
        &f.workspace,
        &[
            "worktree",
            "add",
            "--orphan",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ],
    );
    let mut h = host;
    h.expected_generation = 1;
    h.attachment_request_id = util::id();
    let b = Service::new(a.store.clone(), Some(h));
    let cb = open(&b, &linked, "continue");
    assert_eq!(ca["workflow_id"], cb["workflow_id"]);
    assert_ne!(ca["workspace_id"], cb["workspace_id"]);
    let mut retry = input;
    retry["context_id"] = cb["context_id"].clone();
    let replay = call(&b, "checkpoint_save", retry);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["result"], committed["result"]);
}
#[test]
fn equivalent_scope_paths_cannot_create_multiple_knowledge_identities() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let make = |path: &str| json!({"op":"propose","context_id":c["context_id"],"request_id":util::id(),"expected":"absent","key":"same-key","scope":{"workspace_id":null,"path_prefix":path,"conditions":[]},"claim":"same-scope","evidence_refs":[],"new_evidence":[]});
    call(&s, "knowledge_write", make("src/a"));
    for path in ["src//a", "src/./a"] {
        error(
            s.call("knowledge_write", make(path)),
            ErrorCode::VersionConflict,
        );
    }
}
#[test]
fn frozen_caller_observation_is_not_replaced_by_service_observation() {
    let f = Fixture::new();
    std::fs::write(f.workspace.join("file.txt"), "at-commit").unwrap();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let mut input = save_input(&c, &t);
    input["snapshot"]["workspace"]["related_files"] = json!([{"path":"file.txt","sha256":{"state":"known","value":util::hash("historical-context")},"observed_at":util::now()}]);
    let original = input["snapshot"].clone();
    call(&s, "checkpoint_save", input);
    let restored = get_resume(&s, &c, &t);
    assert_eq!(restored["saved_context"]["snapshot"], original);
    assert_eq!(
        restored["saved_context"]["service_workspace_observation"]["related_files"][0]["sha256"]["value"],
        util::hash("at-commit")
    );
    assert!(
        restored["current_delta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["kind"] == "changed")
    );
}
#[test]
fn largest_escaped_snapshot_has_retrievable_full_pointer() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let mut input = save_input(&c, &t);
    input["snapshot"]["progress"]=json!((0..55).map(|_|json!({"description":"\u{0001}".repeat(1500),"state":"executed_unverified","artifacts":[]})).collect::<Vec<_>>());
    assert!(serde_json::to_vec(&input["snapshot"]).unwrap().len() < 524288);
    let saved = call(&s, "checkpoint_save", input);
    let id = saved["result"]["checkpoint_id"].clone();
    let e=s.call("memory_get",json!({"context_id":c["context_id"],"ids":[id],"revision":null,"cursor":null,"byte_budget":480000})).unwrap_err();
    assert!(matches!(e.code, ErrorCode::BudgetTooSmall));
    assert_eq!(e.full_result_pointer.unwrap().0, id.as_str().unwrap());
    let required = e.required_bytes.unwrap() as usize;
    let full = call(
        &s,
        "memory_get",
        json!({"context_id":c["context_id"],"ids":[id],"revision":null,"cursor":null,"byte_budget":1048576}),
    );
    assert_eq!(util::payload_bytes(&full).unwrap(), required);
    assert!(
        util::payload_bytes(&rmcp::model::CallToolResult::structured(full.clone())).unwrap()
            > 1048576
    );
    assert_eq!(
        full["records"][0]["snapshot"]["progress"]
            .as_array()
            .unwrap()
            .len(),
        55
    );
    assert_eq!(
        full["records"][0]["service_workspace_observation"],
        saved["result"]["workspace_observation"]
    );
}

#[test]
fn executable_knowledge_tool_has_mcp_object_root_schema() {
    let tool = retenixa::mcp::RetenixaServer::knowledge_write_tool_attr();
    assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
    assert!(tool.input_schema.contains_key("oneOf"));
    assert_eq!(
        tool.output_schema.unwrap().get("type"),
        Some(&json!("object"))
    );
}

#[test]
fn explicit_data_dir_does_not_require_ambient_localappdata() {
    let f = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_retenixa"))
        .arg("doctor")
        .arg("--data-dir")
        .arg(&f.data)
        .env_remove("LOCALAPPDATA")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!f.data.exists());
}

#[test]
fn transaction_schema_guard_rechecks_after_connection_was_opened() {
    let f = Fixture::new();
    let store = f.store();
    let mut old = store.connect().unwrap();
    store
        .connect()
        .unwrap()
        .pragma_update(None, "user_version", retenixa::storage::SCHEMA_VERSION + 1)
        .unwrap();
    let tx = old
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(matches!(
        retenixa::storage::assert_schema(&tx).unwrap_err().code,
        ErrorCode::SchemaUnsupported
    ));
}

#[test]
fn semantic_input_validation_and_newer_schema_fail_closed() {
    let f = Fixture::new();
    let (s, c) = f.fresh();
    let t = create(&s, &c, "current_workflow");
    let mut input = save_input(&c, &t);
    input["snapshot"]["workspace"]["observed_at"] = json!("not-a-time");
    error(s.call("checkpoint_save", input), ErrorCode::InvalidInput);
    let mut input = save_input(&c, &t);
    input["expected_revision"] = json!(9007199254740992_i64);
    error(s.call("checkpoint_save", input), ErrorCode::InvalidInput);
    s.store
        .connect()
        .unwrap()
        .pragma_update(None, "user_version", retenixa::storage::SCHEMA_VERSION + 1)
        .unwrap();
    assert!(matches!(
        s.store.connect().unwrap_err().code,
        ErrorCode::SchemaUnsupported
    ));
}
