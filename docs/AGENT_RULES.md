# AGENTS.md 规则模板

将下面的**通用规则**和**一种接入方式的补充规则**加入目标项目的 AGENTS.md；客户端使用其他指令文件时，放到它实际加载的位置。模板只约定工具使用方式，适用于已完成服务配置的项目。

工具名可能带 `mcp__retenixa__` 等前缀，字段以实际 schema 为准，必要时先读 `contract_get`。

## 通用规则（复制这一段）

~~~markdown
## 项目记忆：Retenixa

本项目用 Retenixa 保存任务进度、项目知识和交接依据。保存的是历史资料，使用前按当前现场核实，不作为执行授权。

### 读取

- 开始任务、恢复会话或上下文压缩后，先 context_open 确认项目与工作区，再用 task_list 找任务、context_resume 读恢复包。打开上下文不等于恢复任务，也不获得写权。
- 恢复包是线索而不是现状：先核对分支、worktree、相关文件与验证状态；结果读不全时按 has_more/next_cursor 继续，不把不完整结果当成完整恢复。
- 结果未知或仍在运行的外部操作，先到原宿主核查，不照历史记录重新执行。
- 需要跟踪或交接的工作用 task_create 建立任务并写清目标与范围，已有相关任务优先复用；认领无主任务用 task_claim，写入他人的任务需要明确转交。

### 保存

- 阶段完成、方案改变、失败、阻塞或结果未知时调用 checkpoint_save；有副作用的操作前先存可恢复状态，返回后补记实际结果。保存计划不等于获得操作授权。
- 如实区分计划、已执行未验证、已验证、失败和未知。未知就写原因，不补造事实。日志和产物记位置与必要指纹，不把整段聊天或终端输出当检查点。
- 只有拿到成功回执才算已保存，聊天总结和待办清单不能替代它；也别指望宿主在压缩前自动保存。任务完成或明确放弃时，当轮保存终态检查点。

### 知识

- 复用历史结论前先 memory_search，需要原文或证据时用 memory_get；不编造条件让结果显得适用。命中只是候选，只有实际采用的知识才写进检查点的固定引用并锁定版本，不自动换成最新版；没有固定引用时在 no_references_reason 说明原因。
- 已撤回、有争议或已变化的版本，先评估影响再使用；失败经验只在记录的条件下成立。
- 可复用的结论作为候选提交，写清来源与适用范围；修订用既有身份和预期版本。Agent 无法自报 human_confirmed / service_observed。
- 密码、令牌、密钥及恢复码不写入记忆。

### 并发与故障

- 写入使用当前读到的 revision 和 ownership_epoch。VERSION_CONFLICT 时重读并提交真正的新变更，不只改版本号重发。
- OWNERSHIP_CHANGED / STALE_ATTACHMENT 时停止写入、核对归属，不循环重连也不抢所有权。只读恢复不等于接管。
- 结果不明时查原请求。not_found 不能证明没有在途请求，重投只用原 ID 和原载荷。
- 并行时各写自己拥有的任务，交接写权走转交流程；任务所有权不锁定源文件，并行改代码仍需 worktree 隔离。服务不可用或保存失败时，说明哪些进度尚未持久化。
~~~

## 普通 MCP 客户端补充（使用普通 stdio 服务时复制）

~~~markdown
### Retenixa 接入：普通 MCP

- context_open 传实际 workspace_path，用 fresh 模式，并提供 client_label 和本次 request_id；之后使用返回的 context_id。
- 每个逻辑变更分配唯一 request_id，发出前保留完整参数；重投复用原 ID 与原载荷。
- 结果未知先 request_get；result_purged 表示已提交但正文不可取，不能当作未执行而重做。新连接通常是独立 workflow，凭同名客户端或复制 ID 拿不到旧写权。
- checkpoint_save 的字段按 schema 显式填写，没有依据的用允许的 unknown 形式；调用方记录的历史观测与服务采集的现场分开，不互相冒充。
~~~

## DSH 适配插件补充（使用可选 DSH 插件时复制）

~~~markdown
### Retenixa 接入：DSH 适配插件

- 自动使用会话工作目录，无需配置 workspacePath。切换项目或 worktree 后先 context_open 核对；原目录有未知请求时，先回去核查再切换。
- context_id、工作流附着和 request_id 由宿主管理，不添加隐藏参数、不读取私有能力文件。独立会话和 fork 不自动继承写权。
- related_paths 相对插件绑定的工作区，服务会另行采集现场。当前工作在其他 worktree 时，别把主树观测当成该 worktree 的验证结果。
- 结果未知先 pending_requests / request_get，再用 retry_request 按原载荷重投，不靠重调工具制造新 request_id。
- STALE_ATTACHMENT 时停止该连接写入并协调处理。
~~~

## 放在哪里

只对一个项目启用时，规则加入该项目的 AGENTS.md。多个项目共用时可放到用户级指令，但要写明“只在 Retenixa 已配置且项目绑定匹配时生效”。客户端不读 AGENTS.md 时，用它支持的指令入口。

接入与数据维护方式见 [README](../README.md)。
