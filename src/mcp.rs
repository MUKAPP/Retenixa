use crate::{
    contracts::*,
    service::{HostAttachment, Service},
    storage::Store,
    util,
};
use rmcp::{
    Json, ServerHandler, handler::server::wrapper::Parameters, model::CallToolResult, tool,
    tool_handler, tool_router,
};
#[derive(Clone)]
pub struct RetenixaServer {
    service: Service,
}
#[tool_router]
impl RetenixaServer {
    pub fn new(store: Store, host: Option<HostAttachment>) -> Self {
        Self {
            service: Service::for_stdio(store, host),
        }
    }
    #[tool(
        description = "获取当前可执行与后续计划工具契约",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn contract_get(&self) -> Json<ContractCatalog> {
        Json(crate::contracts::catalog())
    }
    #[tool(description="检查真实 SQLite/FTS5/schema、完整性、当前代次及固定引用缺口，不输出记忆正文",output_schema=rmcp::handler::server::tool::schema_for_output::<crate::maintenance::DoctorOutput>(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn system_doctor(&self) -> CallToolResult {
        let root = self.service.store.root.clone();
        finish(tokio::task::spawn_blocking(move || crate::maintenance::doctor(&root)).await)
    }
    #[tool(description="按当前项目、工作区、任务、路径和条件检索知识及失败尝试；文本查询不执行 FTS 表达式，结果需明确采用才能成为固定依据",output_schema=rmcp::handler::server::tool::schema_for_output::<MemorySearchOutput<'static>>(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn memory_search(
        &self,
        Parameters(input): Parameters<MemorySearchInput>,
    ) -> CallToolResult {
        match util::json(&input) {
            Ok(v) => self.dispatch("memory_search", v).await,
            Err(e) => failure(e),
        }
    }
    async fn dispatch(&self, name: &'static str, input: serde_json::Value) -> CallToolResult {
        let service = self.service.clone();
        finish(tokio::task::spawn_blocking(move || service.call(name, input)).await)
    }
    #[tool(description="打开显式项目和工作流；continue 必须有可信宿主绑定，不凭模型 ID 接管",output_schema=rmcp::handler::server::tool::schema_for_output::<ContextOpenOutput>(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn context_open(
        &self,
        Parameters(input): Parameters<ContextOpenInput>,
    ) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("context_open", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="只读恢复选定检查点、固定依据及当前变化；按 view 和单份 JSON 字节预算返回，查看 omitted_sections；不认领、不执行历史命令",output_schema=rmcp::handler::server::tool::schema_for_output::<ResumePackage>(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn context_resume(
        &self,
        Parameters(input): Parameters<ContextResumeInput>,
    ) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("context_resume", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="建立轻量任务；显式归属当前 workflow 或暂不认领",output_schema=receipt_output_schema(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn task_create(&self, Parameters(input): Parameters<TaskCreateInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("task_create", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="原子认领无主任务；普通本机 stdio 也可接管已确认退出的普通 stdio 所有者，其余须明确管理转交",output_schema=receipt_output_schema(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn task_claim(&self, Parameters(input): Parameters<TaskClaimInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("task_claim", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="由当前所有者显式释放非终态任务；更新 revision/epoch，其他工作流可重新认领",output_schema=receipt_output_schema(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn task_release(&self, Parameters(input): Parameters<TaskClaimInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("task_release", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="按 revision、epoch 和当前附着保存不可变完整快照、固定引用和结案",output_schema=receipt_output_schema(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn checkpoint_save(
        &self,
        Parameters(input): Parameters<CheckpointSaveInput>,
    ) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("checkpoint_save", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="知识版本与来源基础写入；依据政策接受，禁止伪造服务/人工观测",input_schema=knowledge_input_schema(),output_schema=receipt_output_schema(),annotations(read_only_hint=false,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn knowledge_write(
        &self,
        Parameters(input): Parameters<KnowledgeWriteInput>,
    ) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("knowledge_write", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="仅确认当前项目和 workflow 的历史请求结果，不执行或取得写权",output_schema=receipt_output_schema(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn request_get(&self, Parameters(input): Parameters<RequestGetInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("request_get", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="精确读取当前项目的历史记录、来源与快照；显式返回 missing/purged",output_schema=rmcp::handler::server::tool::schema_for_output::<MemoryGetOutput<'static>>(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn memory_get(&self, Parameters(input): Parameters<MemoryGetInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("memory_get", value).await,
            Err(error) => failure(error),
        }
    }
    #[tool(description="按稳定任务 ID 返回当前项目索引与绑定查询的分页游标",output_schema=rmcp::handler::server::tool::schema_for_output::<TaskListOutput>(),annotations(read_only_hint=true,destructive_hint=false,idempotent_hint=true,open_world_hint=false))]
    async fn task_list(&self, Parameters(input): Parameters<TaskListInput>) -> CallToolResult {
        match util::json(&input) {
            Ok(value) => self.dispatch("task_list", value).await,
            Err(error) => failure(error),
        }
    }
}
fn knowledge_input_schema() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let mut schema = rmcp::handler::server::tool::schema_for_type::<KnowledgeWriteInput>()
        .as_ref()
        .clone();
    schema.insert("type".into(), serde_json::json!("object"));
    std::sync::Arc::new(schema)
}
fn receipt_output_schema() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let mut schema = rmcp::handler::server::tool::schema_for_output::<RequestReceipt>()
        .as_ref()
        .clone();
    schema.insert("type".into(), serde_json::json!("object"));
    std::sync::Arc::new(schema)
}
fn finish(
    result: std::result::Result<crate::error::Result<serde_json::Value>, tokio::task::JoinError>,
) -> CallToolResult {
    match result {
        Ok(Ok(v)) => CallToolResult::structured(v),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(DomainError::new(
            ErrorCode::StorageFailure,
            "阻塞工作线程失败",
        )),
    }
}
fn failure(error: DomainError) -> CallToolResult {
    let mut result = CallToolResult::structured(serde_json::json!({"error":error}));
    result.is_error = Some(true);
    result
}
#[tool_handler(
    name = "retenixa",
    instructions = "跟踪或接续任务 → context_open；纯只读问答或无需接续的小改动，不建任务、不存检查点。context_resume 只读。归属冲突 → 停止写入，列出交接信息，请用户决定，勿静默另建。未知写入 → 查原 request_id；未知外部操作 → 查原宿主，勿重跑。"
)]
impl ServerHandler for RetenixaServer {}
