//! 公共输入、固定快照和恢复契约；领域服务执行交叉字段和事务约束。
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Identifier(#[schemars(length(min = 1, max = 128))] pub String);

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Text(#[schemars(length(min = 1, max = 16384))] pub String);

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Timestamp(#[schemars(extend("format" = "date-time"), regex(pattern = "Z$"))] pub String);

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Sha256(#[schemars(regex(pattern = "^[a-f0-9]{64}$"))] pub String);

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Revision(#[schemars(range(min = 1, max = 9007199254740991_u64))] pub i64);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observed<T> {
    Known { value: T },
    Unknown { reason: Text },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    InProgress,
    Blocked,
    Completed,
    Abandoned,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Planned,
    ExecutedUnverified,
    Verified,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStatus {
    Candidate,
    Active,
    Disputed,
    Superseded,
    Retracted,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    Match,
    NeedsReview,
    Mismatch,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceObservation {
    pub workspace_id: Identifier,
    pub observed_at: Timestamp,
    pub branch: Observed<String>,
    pub base_commit: Observed<String>,
    pub head: Observed<String>,
    pub related_files: Vec<FileObservation>,
    pub consistency: ObservationConsistency,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationConsistency {
    ConsistentForObservedFiles,
    ChangedDuringObservation,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileObservation {
    pub path: Text,
    pub sha256: Observed<Sha256>,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgressRecord {
    pub description: Text,
    pub state: ExecutionState,
    pub artifacts: Vec<Text>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationRecord {
    pub description: Text,
    pub state: ExecutionState,
    pub result: Text,
    pub evidence_ids: Vec<Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub goal: Text,
    pub conditions: Vec<Text>,
    pub method: Text,
    pub state: ExecutionState,
    pub result: Text,
    pub reason: Observed<Text>,
    pub verification_refs: Vec<Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnresolvedOperation {
    pub host: Text,
    pub operation_id: Identifier,
    pub description: Text,
    pub state: UnresolvedState,
    pub possible_side_effects: Vec<Text>,
    pub verification_method: Text,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedState {
    InProgress,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSelection {
    CallerExplicit,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeReference {
    pub knowledge_id: Identifier,
    pub revision: Revision,
    pub purpose: Text,
    pub content_sha256: Option<Sha256>,
    pub selection: ReferenceSelection,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub evidence_id: Identifier,
    pub source_revision: Revision,
    pub content_sha256: Sha256,
    pub observed_at: Timestamp,
    pub purpose: Text,
    pub selection: ReferenceSelection,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointSnapshot {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    pub goal: Text,
    pub scope: Text,
    pub stage: Text,
    pub completion_conditions: Vec<Text>,
    pub workspace: WorkspaceObservation,
    pub progress: Vec<ProgressRecord>,
    pub attempts: Vec<Attempt>,
    pub verifications: Vec<VerificationRecord>,
    pub unresolved_operations: Vec<UnresolvedOperation>,
    pub blockers: Vec<Text>,
    pub next_steps: Vec<Text>,
    /// knowledge_refs 和 evidence_refs 均为空时必填，说明本检查点为何没有固定依据。
    pub no_references_reason: Option<Text>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    Fresh,
    Continue,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOpenInput {
    pub workspace_path: Text,
    #[schemars(length(min = 1, max = 128))]
    pub client_label: String,
    pub mode: OpenMode,
    pub request_id: Identifier,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOpenOutput {
    pub project_id: Identifier,
    pub workspace_id: Identifier,
    pub workflow_id: Identifier,
    pub session_id: Identifier,
    pub context_id: Identifier,
    pub writer_generation: Revision,
    pub replayed: bool,
    pub requires_reattach: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeView {
    Compact,
    #[default]
    Standard,
    Full,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextResumeInput {
    pub context_id: Identifier,
    pub task_id: Identifier,
    pub checkpoint_id: Option<Identifier>,
    #[serde(default)]
    pub view: ResumeView,
    #[schemars(length(max = 4096))]
    pub cursor: Option<String>,
    #[schemars(range(min = 1, max = 4194304))]
    pub byte_budget: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskCreateInput {
    pub context_id: Identifier,
    pub request_id: Identifier,
    pub goal: Text,
    pub scope: Text,
    pub assignment: Assignment,
    pub parent_task_id: Option<Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Assignment {
    CurrentWorkflow,
    Unassigned,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskClaimInput {
    pub context_id: Identifier,
    pub request_id: Identifier,
    pub task_id: Identifier,
    pub expected_revision: Revision,
    pub expected_epoch: Revision,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointSaveInput {
    pub context_id: Identifier,
    pub request_id: Identifier,
    pub task_id: Identifier,
    pub expected_revision: Revision,
    pub ownership_epoch: Revision,
    pub snapshot: CheckpointSnapshot,
    pub knowledge_refs: Vec<KnowledgeReference>,
    pub evidence_refs: Vec<EvidenceReference>,
    pub target_status: TaskStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Knowledge,
    Attempts,
    All,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchScope {
    pub task_id: Option<Identifier>,
    pub workspace_id: Option<Identifier>,
    pub path_prefix: Option<Text>,
    #[serde(default)]
    pub include_inactive: bool,
    #[serde(default)]
    pub conditions: Vec<Text>,
}

fn default_search_limit() -> u32 {
    20
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySearchInput {
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: u32,
    pub context_id: Identifier,
    /// 普通文本。先匹配去停用词后的全部词，再用部分匹配补齐；短词和无 FTS 命中时扫描字面文本。
    #[schemars(length(min = 1, max = 4096))]
    pub query: String,
    pub kind: SearchKind,
    pub scope: SearchScope,
    #[schemars(length(max = 4096))]
    pub cursor: Option<String>,
    #[schemars(range(min = 1, max = 4194304))]
    pub byte_budget: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryGetInput {
    pub context_id: Identifier,
    #[schemars(length(min = 1, max = 100))]
    pub ids: Vec<Identifier>,
    pub revision: Option<Revision>,
    #[schemars(length(max = 4096))]
    pub cursor: Option<String>,
    #[schemars(range(min = 1, max = 4194304))]
    pub byte_budget: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestGetInput {
    pub context_id: Identifier,
    pub request_id: Identifier,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestReceipt {
    Committed {
        request_id: Identifier,
        committed_at: Timestamp,
        committed_versions: Vec<EntityVersion>,
        result: Value,
        replayed: bool,
    },
    NotFound {
        request_id: Identifier,
    },
    ResultPurged {
        request_id: Identifier,
        reason: Text,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityVersion {
    pub entity_id: Identifier,
    pub revision: Revision,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskListInput {
    pub context_id: Identifier,
    pub status: Option<TaskStatus>,
    pub workspace_id: Option<Identifier>,
    pub parent_task_id: Option<Identifier>,
    #[schemars(length(max = 4096))]
    pub cursor: Option<String>,
    #[schemars(range(min = 1, max = 100))]
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeScope {
    pub workspace_id: Option<Identifier>,
    pub path_prefix: Option<Text>,
    pub conditions: Vec<Text>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceBasis {
    pub rationale: Text,
    pub evidence_refs: Vec<EvidenceReference>,
    /// 归因报告由服务按当前 workflow 记录，不能自报 human_confirmed/service_observed。
    pub agent_report: Option<Text>,
}

/// 新证据统一归因为 agent_reported，调用方不能指定 human_confirmed 或 service_observed。
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentEvidenceInput {
    pub local_key: Identifier,
    pub source: Text,
    pub observation: Text,
    pub content_sha256: Sha256,
    pub observed_at: Timestamp,
    pub origin_observation_id: Option<Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CreationExpectation {
    Absent,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct SearchTag(#[schemars(length(min = 1, max = 64))] pub String);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeWriteInput {
    Propose {
        context_id: Identifier,
        request_id: Identifier,
        expected: CreationExpectation,
        key: Identifier,
        scope: KnowledgeScope,
        claim: Text,
        /// 最多 8 个同义词或症状标签，只用于检索，不作为证据或适用条件。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schemars(length(max = 8))]
        search_tags: Option<Vec<SearchTag>>,
        evidence_refs: Vec<EvidenceReference>,
        new_evidence: Vec<AgentEvidenceInput>,
    },
    Accept {
        context_id: Identifier,
        request_id: Identifier,
        knowledge_id: Identifier,
        expected_revision: Revision,
        acceptance_basis: AcceptanceBasis,
    },
    Revise {
        context_id: Identifier,
        request_id: Identifier,
        knowledge_id: Identifier,
        expected_revision: Revision,
        claim: Text,
        /// 最多 8 个检索标签。省略则沿用上个版本，空数组清除标签。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schemars(length(max = 8))]
        search_tags: Option<Vec<SearchTag>>,
        evidence_refs: Vec<EvidenceReference>,
        new_evidence: Vec<AgentEvidenceInput>,
        reason: Text,
    },
    Dispute {
        context_id: Identifier,
        request_id: Identifier,
        knowledge_id: Identifier,
        expected_revision: Revision,
        reason: Text,
    },
    Supersede {
        context_id: Identifier,
        request_id: Identifier,
        knowledge_id: Identifier,
        expected_revision: Revision,
        replacement_id: Identifier,
        replacement_expected_revision: Revision,
        reason: Text,
    },
    Retract {
        context_id: Identifier,
        request_id: Identifier,
        knowledge_id: Identifier,
        expected_revision: Revision,
        reason: Text,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurrentTask {
    pub task_id: Identifier,
    pub revision: Revision,
    pub ownership_epoch: Revision,
    pub owner_workflow_id: Option<Identifier>,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedContext {
    pub checkpoint_id: Identifier,
    pub checkpoint_sequence: Revision,
    pub task_revision_at_save: Revision,
    pub snapshot: CheckpointSnapshot,
    pub service_workspace_observation: WorkspaceObservation,
    pub knowledge_refs: Vec<KnowledgeReference>,
    pub evidence_refs: Vec<EvidenceReference>,
    /// 精确历史正文；缺失/清除必须在 current_delta 和 resume_requirements 中出现。
    pub fixed_knowledge: Vec<FixedKnowledge>,
    pub fixed_evidence: Vec<FixedEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixedKnowledge {
    pub knowledge_id: Identifier,
    pub revision: Revision,
    pub claim: Text,
    pub content_sha256: Sha256,
    pub status: KnowledgeStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    AgentReported,
    ServiceObserved,
    Imported,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixedEvidence {
    pub evidence_id: Identifier,
    pub source_revision: Revision,
    pub source: Text,
    pub source_kind: SourceKind,
    pub content_sha256: Sha256,
    pub observed_at: Timestamp,
    pub origin_observation_id: Identifier,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Changed,
    Retracted,
    Disputed,
    Missing,
    Purged,
    Unknown,
    OwnershipChanged,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextDelta {
    pub kind: DeltaKind,
    pub subject_id: Identifier,
    pub saved_revision: Option<Revision>,
    pub current_revision: Option<Revision>,
    pub reason: Text,
    pub applicability: Applicability,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeRequirement {
    pub subject_id: Identifier,
    pub action: Text,
    pub available_evidence: Vec<Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumePackage {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    pub returned_at: Timestamp,
    pub saved_context: SavedContext,
    pub current_task: CurrentTask,
    pub current_workspace_observation: Observed<WorkspaceObservation>,
    pub current_delta: Vec<ContextDelta>,
    pub resume_requirements: Vec<ResumeRequirement>,
    pub view: ResumeView,
    pub related_context: Vec<RelatedKnowledge>,
    pub fixed_content_page: FixedContentPage,
    pub omitted_sections: Vec<ResumeSection>,
    pub omitted_categories: Vec<Text>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeSection {
    RelatedContext,
    FixedReferenceBodies,
    HistoricalAttempts,
    HistoricalVerifications,
    ReferencePurposes,
    FixedReferences,
    WorkspaceFileObservations,
    SnapshotMetadata,
    SnapshotNotes,
    DerivedRequirements,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelatedKnowledge {
    pub id: Identifier,
    pub revision: Revision,
    pub key: Identifier,
    pub match_reason: Text,
    pub applicability: Applicability,
    pub excerpt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixedContentPage {
    pub offset: u32,
    pub returned: u32,
    pub total: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidInput,
    ProjectScopeMismatch,
    NotFound,
    VersionConflict,
    StaleAttachment,
    OwnershipChanged,
    ClaimDenied,
    RequestIdReused,
    ReferenceUnavailable,
    StorageBusy,
    StorageFailure,
    SchemaUnsupported,
    BudgetTooSmall,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DomainError {
    pub code: ErrorCode,
    pub message: Text,
    pub recovery_hint: Text,
    pub required_bytes: Option<u64>,
    pub full_result_pointer: Option<Identifier>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub budget_details: Option<Box<BudgetDetails>>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub ownership_details: Option<Box<OwnershipDetails>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BudgetDetails {
    pub min_required_bytes: u64,
    pub largest_fields: Vec<BudgetField>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BudgetField {
    pub field: String,
    pub bytes: u64,
    pub percent: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnershipDetails {
    pub task_id: Identifier,
    pub current_revision: Revision,
    pub current_epoch: Revision,
    pub owner_workflow_id: Option<Identifier>,
}

#[derive(Serialize, JsonSchema)]
pub struct ContractCatalog {
    pub contract_version: String,
    pub executable_tools: Vec<String>,
    pub planned_tools: BTreeMap<String, Value>,
    pub tool_schemas: BTreeMap<String, Value>,
    pub data_schemas: BTreeMap<String, Value>,
}

fn schema<T: JsonSchema>() -> Value {
    schema_for!(T).to_value()
}

pub fn catalog() -> ContractCatalog {
    let tool_schemas = BTreeMap::from([
        ("context_open".into(), schema::<ContextOpenInput>()),
        ("context_resume".into(), schema::<ContextResumeInput>()),
        ("task_create".into(), schema::<TaskCreateInput>()),
        ("task_claim".into(), schema::<TaskClaimInput>()),
        ("task_release".into(), schema::<TaskClaimInput>()),
        ("checkpoint_save".into(), schema::<CheckpointSaveInput>()),
        ("knowledge_write".into(), schema::<KnowledgeWriteInput>()),
        ("memory_search".into(), schema::<MemorySearchInput>()),
        ("memory_get".into(), schema::<MemoryGetInput>()),
        ("request_get".into(), schema::<RequestGetInput>()),
        ("task_list".into(), schema::<TaskListInput>()),
    ]);
    ContractCatalog {
        contract_version: "0.5.0".into(),
        executable_tools: [
            "contract_get",
            "system_doctor",
            "context_open",
            "context_resume",
            "task_create",
            "task_claim",
            "task_release",
            "checkpoint_save",
            "knowledge_write",
            "memory_get",
            "memory_search",
            "request_get",
            "task_list",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        planned_tools: BTreeMap::new(),
        tool_schemas,
        data_schemas: BTreeMap::from([
            ("context_open_output".into(), schema::<ContextOpenOutput>()),
            ("checkpoint_snapshot".into(), schema::<CheckpointSnapshot>()),
            ("resume_package".into(), schema::<ResumePackage>()),
            ("request_receipt".into(), schema::<RequestReceipt>()),
            ("domain_error".into(), schema::<DomainError>()),
        ]),
    }
}
