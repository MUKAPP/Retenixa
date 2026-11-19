use super::*;
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Write, path::Path};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostAttachment {
    pub schema_version: u32,
    pub host: String,
    pub host_session: String,
    pub capability: String,
    pub attachment_request_id: String,
    pub expected_generation: i64,
}
impl HostAttachment {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        if bytes.len() > 8192 {
            return Err(DomainError::invalid("宿主绑定文件过大"));
        }
        let h: Self = serde_json::from_slice(&bytes)
            .map_err(|_| DomainError::invalid("宿主私有绑定文件格式错误（正文不输出）"))?;
        if h.schema_version != 1
            || h.host.trim().is_empty()
            || h.host_session.trim().is_empty()
            || h.capability.len() != 64
            || !h.capability.bytes().all(|x| x.is_ascii_hexdigit())
            || h.attachment_request_id.is_empty()
            || !(0..=9007199254740991).contains(&h.expected_generation)
        {
            return Err(DomainError::invalid("宿主私有绑定无效"));
        }
        Ok(h)
    }
    pub fn ensure(path: &Path, host: &str, session: &str) -> Result<()> {
        if !path.is_absolute() || host.trim().is_empty() || session.trim().is_empty() {
            return Err(DomainError::invalid(
                "宿主初始化需要绝对绑定文件路径和可信宿主身份",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| DomainError::invalid("绑定路径无父目录"))?;
        crate::permissions::secure_dir(parent)?;
        if path.exists() {
            let old = Self::load(path)?;
            if old.host != host || old.host_session != session {
                return Err(DomainError::invalid("已有绑定属于不同宿主会话"));
            }
            return Ok(());
        }
        let h = Self {
            schema_version: 1,
            host: host.into(),
            host_session: session.into(),
            capability: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            attachment_request_id: util::id(),
            expected_generation: 0,
        };
        let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
        crate::permissions::secure_file(path)?;
        f.write_all(&serde_json::to_vec(&h)?)?;
        f.sync_all()?;
        Ok(())
    }
    fn hash(&self) -> Result<String> {
        Ok(util::hash(serde_json::to_vec(&(
            &self.host,
            &self.host_session,
            &self.capability,
        ))?))
    }
}
impl Service {
    fn register_location(
        tx: &Transaction<'_>,
        location: &crate::project::Location,
    ) -> Result<(String, String)> {
        let old = tx
            .query_row(
                "SELECT project_id,id,identity_key FROM workspaces WHERE root=?1",
                [&location.root],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let mut project = tx
            .query_row(
                "SELECT project_id FROM project_locations WHERE identity_key=?1",
                [&location.identity],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        // A linked worktree can be the first connection after git init.
        if old.is_none()
            && location.git
            && let Some(root) = crate::project::standard_main_root(location)?
            && root != location.root
        {
            let main = tx
                .query_row(
                    "SELECT project_id,id,identity_key FROM workspaces WHERE root=?1",
                    [&root],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((p, w, key)) = main
                && key == format!("path:{root}")
            {
                Self::upgrade_location(tx, &p, &w, &root, &key, location, project.as_deref())?;
                project = Some(p);
            }
        }
        if let Some((p, w, key)) = &old
            && key != &location.identity
        {
            Self::upgrade_location(tx, p, w, &location.root, key, location, project.as_deref())?;
            project = Some(p.clone());
        }
        let project = match project {
            Some(p) => p,
            None => {
                let p = util::id();
                tx.execute(
                    "INSERT INTO projects(id,created_at) VALUES(?1,?2)",
                    params![p, util::now()],
                )?;
                tx.execute(
                    "INSERT INTO project_locations(identity_key,project_id) VALUES(?1,?2)",
                    params![location.identity, p],
                )?;
                p
            }
        };
        if let Some((p, w, _)) = old {
            if p != project {
                return Err(DomainError::new(
                    ErrorCode::ProjectScopeMismatch,
                    format!(
                        "工作区绑定冲突：source_project_id={p}, target_project_id={project}；使用离线 project-merge --request-file 维护入口"
                    ),
                ));
            }
            return Ok((p, w));
        }
        let workspace = util::id();
        tx.execute(
            "INSERT INTO workspaces VALUES(?1,?2,?3,?4)",
            params![project, workspace, location.root, location.identity],
        )?;
        Ok((project, workspace))
    }
    fn upgrade_location(
        tx: &Transaction<'_>,
        project: &str,
        workspace: &str,
        root: &str,
        old_identity: &str,
        location: &crate::project::Location,
        target: Option<&str>,
    ) -> Result<()> {
        if !location.git
            || old_identity != format!("path:{root}")
            || target.is_some_and(|p| p != project)
        {
            return Err(DomainError::new(
                ErrorCode::ProjectScopeMismatch,
                format!(
                    "工作区身份变化：source_project_id={project}, target_project_id={}；仅支持精确目录身份升级，已有分裂项目使用离线 project-merge --request-file",
                    target.unwrap_or("unregistered")
                ),
            ));
        }
        let alias: Option<String> = tx
            .query_row(
                "SELECT project_id FROM project_locations WHERE identity_key=?1",
                [old_identity],
                |r| r.get(0),
            )
            .optional()?;
        if alias.as_deref() != Some(project) {
            return Err(DomainError::new(
                ErrorCode::ProjectScopeMismatch,
                "旧目录 alias 与 workspace 项目不一致",
            ));
        }
        crate::project_merge::preserve_attachment_location(tx, project, workspace, old_identity)?;
        tx.execute(
            "UPDATE workspaces SET identity_key=?1 WHERE project_id=?2 AND id=?3",
            params![location.identity, project, workspace],
        )?;
        if target.is_none() {
            tx.execute(
                "INSERT INTO project_locations(identity_key,project_id) VALUES(?1,?2)",
                params![location.identity, project],
            )?;
        }
        Ok(())
    }
    pub(super) fn open_context(&self, input: ContextOpenInput) -> Result<Value> {
        if input.client_label.trim().is_empty() || input.client_label.chars().count() > 128 {
            return Err(DomainError::invalid("client_label 为空或超长"));
        }
        let location = crate::project::locate(&input.workspace_path.0)?;
        let hash = util::hash(util::canonical(
            &json!({"location":location.identity,"root":location.root,"client_label":input.client_label,"mode":input.mode}),
        )?);
        let bound = matches!(input.mode, OpenMode::Continue);
        if bound && self.host.is_none() {
            return Err(DomainError::new(
                ErrorCode::ClaimDenied,
                "continue 需要宿主非模型通道的可信绑定",
            ));
        }
        if !bound && self.host.is_some() {
            return Err(DomainError::invalid(
                "已配置可信宿主绑定的连接应使用 continue",
            ));
        }
        let mut conn = self.store.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        crate::storage::assert_schema(&tx)?;
        crate::search::assert_index(&tx)?;
        if !bound
            && let Some((h,b))=tx.query_row("SELECT payload_hash,result_json FROM bootstrap_receipts WHERE connection_id=?1 AND request_id=?2",params![self.connection_id,input.request_id.0],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?{if h!=hash{return Err(DomainError::new(ErrorCode::RequestIdReused,"context_open 请求载荷不一致"));}let mut v:Value=serde_json::from_str(&b)?;v["replayed"]=json!(true);return Ok(v);}
        let (project, workspace) = Self::register_location(&tx, &location)?;
        let binding = self.host.as_ref().map(|h| h.hash()).transpose()?;
        let attachment = self.host.as_ref().map(|h| h.attachment_request_id.as_str());
        if let (Some(binding), Some(request)) = (&binding, attachment)
            && let Some((old_hash,body,hash_location))=tx.query_row("SELECT payload_hash,result_json,hash_location FROM attachment_receipts WHERE project_id=?1 AND binding_hash=?2 AND request_id=?3",params![project,binding,request],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).optional()?{
                 let replay_hash=util::hash(util::canonical(&json!({"location":hash_location.as_deref().unwrap_or(&location.identity),"root":location.root,"client_label":input.client_label,"mode":input.mode}))?);
                if old_hash!=replay_hash{return Err(DomainError::new(ErrorCode::RequestIdReused,"宿主附着请求 ID 用于不同载荷"));}
                let mut v:Value=serde_json::from_str(&body)?;let wf=util::text(&v,"workflow_id");let ctx=util::text(&v,"context_id");
                let (generation,active):(i64,String)=tx.query_row("SELECT generation,active_context_id FROM workflows WHERE project_id=?1 AND id=?2",params![project,wf],|r|Ok((r.get(0)?,r.get(1)?)))?;
                if Some(generation)!=v["writer_generation"].as_i64()||active!=ctx{return Err(DomainError::new(ErrorCode::StaleAttachment,"原附着回执已过期，不恢复旧写权"));}
                let original_connection:String=tx.query_row("SELECT connection_id FROM contexts WHERE project_id=?1 AND id=?2",params![project,ctx],|r|r.get(0))?;v["requires_reattach"]=json!(original_connection!=self.connection_id);
                v["replayed"]=json!(true);tx.commit()?;return Ok(v);
            }
        if let Some((context,workflow,session,generation,old_workspace))=tx.query_row("SELECT c.id,c.workflow_id,c.session_id,c.generation,c.workspace_id FROM contexts c JOIN workflows f ON c.project_id=f.project_id AND c.workflow_id=f.id WHERE c.project_id=?1 AND c.connection_id=?2 AND f.active_context_id=c.id",params![project,self.connection_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?))).optional()? {
            if old_workspace!=workspace{return Err(DomainError::invalid("同一存活连接不能隐式切换已附着的 worktree"));}
            let v=json!({"project_id":project,"workspace_id":workspace,"workflow_id":workflow,"session_id":session,"context_id":context,"writer_generation":generation,"replayed":true,"requires_reattach":false});
            self.save_open_receipt(&tx,&project,binding.as_deref(),&input.request_id.0,&hash,&v)?;tx.commit()?;return Ok(v);
        }
        let context = util::id();
        let session = util::id();
        let (workflow, generation) = if let Some(host) = &self.host {
            let prior = tx
                .query_row(
                    "SELECT id,generation FROM workflows WHERE project_id=?1 AND binding_hash=?2",
                    params![project, binding],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            if let Some((wf, generation)) = prior {
                if generation != host.expected_generation {
                    return Err(DomainError::new(
                        ErrorCode::StaleAttachment,
                        "宿主预期附着代次落后；停止自动重连",
                    ));
                }
                if generation >= 9007199254740991 {
                    return Err(DomainError::invalid("generation 已到上限"));
                }
                let n = generation + 1;
                tx.execute("UPDATE workflows SET generation=?1,active_context_id=?2 WHERE project_id=?3 AND id=?4 AND generation=?5",params![n,context,project,wf,generation])?;
                (wf, n)
            } else {
                let existing_host:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workflows WHERE project_id=?1 AND host_identity=?2 UNION ALL SELECT 1 FROM workflow_host_aliases WHERE project_id=?1 AND host_identity=?2)",params![project,serde_json::to_string(&(&host.host,&host.host_session))?],|r|r.get(0))?;
                if existing_host {
                    return Err(DomainError::new(
                        ErrorCode::ClaimDenied,
                        "该宿主会话已有不同能力绑定；恢复原绑定或明确管理转交，不创建同名可信工作流",
                    ));
                }
                if host.expected_generation != 0 {
                    return Err(DomainError::new(
                        ErrorCode::StaleAttachment,
                        "宿主绑定无法在当前数据库证明连续身份",
                    ));
                }
                let wf = util::id();
                tx.execute(
                    "INSERT INTO workflows VALUES(?1,?2,?3,?4,1,?5)",
                    params![
                        project,
                        wf,
                        binding,
                        serde_json::to_string(&(&host.host, &host.host_session))?,
                        context
                    ],
                )?;
                (wf, 1)
            }
        } else {
            let wf = util::id();
            tx.execute(
                "INSERT INTO workflows VALUES(?1,?2,NULL,NULL,1,?3)",
                params![project, wf, context],
            )?;
            (wf, 1)
        };
        tx.execute(
            "INSERT INTO contexts VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                project,
                context,
                workflow,
                workspace,
                session,
                generation,
                self.connection_id
            ],
        )?;
        let v = json!({"project_id":project,"workspace_id":workspace,"workflow_id":workflow,"session_id":session,"context_id":context,"writer_generation":generation,"replayed":false,"requires_reattach":false});
        self.save_open_receipt(
            &tx,
            &project,
            binding.as_deref(),
            &input.request_id.0,
            &hash,
            &v,
        )?;
        tx.commit()?;
        Ok(v)
    }
    fn save_open_receipt(
        &self,
        tx: &Transaction<'_>,
        project: &str,
        binding: Option<&str>,
        request: &str,
        hash: &str,
        v: &Value,
    ) -> Result<()> {
        if let (Some(binding), Some(host)) = (binding, self.host.as_ref()) {
            let attachment = &host.attachment_request_id;
            tx.execute(
                "INSERT INTO attachment_receipts(project_id,binding_hash,request_id,payload_hash,result_json) VALUES(?1,?2,?3,?4,?5)",
                params![
                    project,
                    binding,
                    attachment,
                    hash,
                    serde_json::to_string(v)?
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO bootstrap_receipts VALUES(?1,?2,?3,?4)",
                params![self.connection_id, request, hash, serde_json::to_string(v)?],
            )?;
        }
        Ok(())
    }
}
