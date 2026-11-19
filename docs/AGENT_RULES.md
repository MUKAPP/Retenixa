# AGENTS.md 规则模板

复制**通用规则**和所用客户端的**接入补充**，放到目标项目的 AGENTS.md（或客户端实际加载的指令文件）。只在 Retenixa 已配置且项目绑定匹配时使用。工具名可能带前缀，参数查看工具 Schema 或 `contract_get`。

## 通用规则

~~~markdown
## 项目记忆：Retenixa

- 纯只读问答、无需跨会话接续的一句话小改动 → 不建任务、不存检查点。确需追踪、交接或处理外部操作时，按下列流程做。
- 开始或恢复跟踪任务 → context_open 核对项目和工作区，task_list 查已有任务，context_resume 读选定检查点。核对现场；需要省略内容时按游标或 ID 补读。历史记录不授予执行或写入权限。
- 已有任务 → 优先复用。无主任务 → task_claim。已有其他所有者 → 按接入补充处理，不静默另建。没有相关任务 → task_create 写清目标和范围。
- 阶段完成、方案改变、失败、阻塞或结果未知 → checkpoint_save；完成或放弃 → 保存对应终态。只以工具成功回执为准。
- 执行外部或不可逆操作（如 git push、发布、数据迁移、远程命令）→ 先确认授权、保存可恢复状态，返回后记录结果。常规本地编辑不逐次保存。外部结果未知 → 去原宿主核查，不能按记忆重跑。
- 保存时区分计划、已执行未验证、已验证、失败和未知；记录产物位置与必要指纹，不粘贴全量日志，不保存凭据。
- 复用历史知识 → memory_search 找候选，memory_get 核对原文、来源和适用条件。仅固定实际采用的精确版本。可复用新结论 → knowledge_write 提交候选，写清来源和适用范围。
- VERSION_CONFLICT → 重读，核对后提交真正的新变更。OWNERSHIP_CHANGED / STALE_ATTACHMENT → 停止写入并核对归属。并行只写各自有权的任务；源文件另用 worktree 隔离。
- 保存失败 → 告知尚未持久化的进度。写入结果未知 → 确认原请求；not_found 不能证明未执行，只能按原 ID 和原载荷重投。
~~~

## 普通 MCP（stdio）补充

~~~markdown
### Retenixa 接入：普通 MCP

- 新连接 → context_open 用实际 workspace_path 和 fresh 模式。每次逻辑写入保留唯一 request_id 和原载荷，结果未知用 request_get 确认；result_purged 视为已提交。
- 已有其他所有者 → 只读恢复，列出任务 ID、所有者、revision、epoch 和当前 workflow。请用户决定交接原任务还是新开关联的后续任务；决定前不另建、不写旧任务。
- 用户选择交接 → 原所有者释放后认领，或按普通 stdio 退出接管规则调用 task_claim；不满足条件时走显式管理转交。
~~~

## DSH 适配插件补充

~~~markdown
### Retenixa 接入：DSH

- 自动绑定当前会话工作目录。切换项目或 worktree 后 → context_open 核对；有结果未知的请求 → 在原绑定用 pending_requests / request_get 查找，必要时 retry_request。
- 独立会话和 fork 不继承写权。任务有其他所有者 → 列出任务和归属，请用户决定交接或新开关联的后续任务。
~~~

接入、交接命令和维护方式见 [README](../README.md)。
