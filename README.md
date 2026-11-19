# Retenixa

**让 Agent 记住项目，并在下一次会话里可靠地接着做。**

Retenixa 是一个本地运行的 MCP 项目记忆服务。它把任务进度、项目知识、失败经验和交接依据存进 SQLite，让不同会话、不同 MCP 客户端共享同一份项目上下文，减少重复调查并保留可追溯的工作依据。

恢复任务时，Retenixa 同时返回**当时保存的上下文**与**现在发生的变化**：上次依据的是哪一版知识、相关文件是否改变、哪些操作还需要确认。多个 Agent 写入时，通过版本和任务归属检查阻止相互覆盖。

## 能做什么

- **接续任务**：保存完整检查点，包含目标、进展、尝试结果、验证情况和下一步。
- **共享项目知识**：按项目、worktree、路径和适用条件检索，中英文及代码标识符均可搜索。
- **保留历史依据**：检查点固定知识与证据的具体版本；知识后来被修订、争议或撤回时，恢复会明确提示。
- **记住失败尝试**：检索曾经试过的方法及其失败条件，避免重复走弯路。
- **协调并发写入**：任务有明确所有者，旧版本写入会被拒绝；结果未知的请求可以按原 ID 确认。

数据保存在本机，不需要云账户、API Key、向量数据库或额外模型服务。Git 仓库的多个 worktree 共享项目身份，但保留各自的工作区信息；非 Git 项目也可使用。

## 快速开始

当前构建与验证平台为 **Windows x86_64**。安装 Rust（工具链由仓库固定为 1.95.0）、MSVC C/C++ Build Tools 和 Git 后，在仓库根目录运行：

~~~bash
cargo build --release --locked
./target/release/retenixa.exe doctor
~~~

生成的可执行文件为 target/release/retenixa.exe。SQLite 随程序内置，无需单独安装。`retenixa.exe --version` 输出核心版本号，可与插件版本分别确认。

接下来选择一种接入方式：普通 MCP 客户端，或带连续身份管理的 DSH 适配插件。

### 普通 MCP 客户端

在客户端中添加一个 **stdio MCP 服务**，command 使用可执行文件的绝对路径，args 指定 serve 和数据目录。

下面是采用 mcpServers 格式的客户端配置示例；请替换示例路径。其他客户端使用相同的 command / args，外层配置结构以客户端要求为准。

~~~json
{
  "mcpServers": {
    "retenixa": {
      "command": "D:/Tools/Retenixa/retenixa.exe",
      "args": ["serve", "--data-dir", "D:/Data/Retenixa"]
    }
  }
}
~~~

客户端会启动服务，不需要另开终端常驻运行 serve。每个客户端可以有自己的进程；**需要共享记忆的客户端必须指向同一个数据目录**。

连接后，让 Agent 调用 context_open，明确传入要工作的项目绝对路径。普通客户端使用 fresh 模式；同一仓库的 worktree 会识别为同一项目。读取旧任务不需要接管，但新工作流要继续写入已有所有者的任务，需要明确转交，见下文。

### DSH 适配插件

DSH 用户可安装仓库中的 [适配插件](adapters/dsh/README.md)。它会自行启动 Retenixa，并替 Agent 管理持久工作流绑定、请求 ID 和结果未知的原始请求；合法重启可以续接原工作流。

在 DSH 插件管理器中安装 adapters/dsh 的本地绝对路径或打包后的 tgz，然后填写：

| 字段 | 内容 |
| --- | --- |
| command | retenixa.exe 的绝对路径 |
| dataDir | 默认为 %LOCALAPPDATA%/Retenixa；与其他客户端共享记忆时使用同一路径 |

在插件详情页的说明下方填写并保存。可执行文件必填，**数据目录已默认取 %LOCALAPPDATA%/Retenixa，与核心服务不带 --data-dir 时的默认值一致，通常无需改动**；保存后插件会重新连接会话。**项目目录自动跟随每个会话的当前工作目录，无需配置 workspacePath**；不同项目的数据自动隔离，同一仓库的 worktree 共享项目记忆。旧版安装若停用了组件，先启用再填写设置。同一作用域使用该插件时，无需再配置一份普通 Retenixa MCP 服务。

安装、支持的宿主版本和配置示例见 [DSH 插件说明](adapters/dsh/README.md)。

## 让 Agent 使用项目记忆

MCP 接入提供工具，还需要告诉 Agent 何时读取、何时保存。把 [AGENTS.md 规则模板](docs/AGENT_RULES.md) 中的“通用规则”和与你的接入方式对应的规则加入目标项目的 AGENTS.md，或加入客户端实际加载的同类指令文件。

日常使用围绕以下流程进行：

纯只读问答和无需接续的一句话小改动可以跳过建任务和存检查点。需要追踪或交接时：

1. **开始或恢复**：打开项目上下文，列出相关任务，读取选定任务的恢复包并核对现场。
2. **工作前查找**：按任务、路径与条件检索知识或失败尝试，必要时读取完整历史版本。
3. **阶段保存**：在完成一个可核对阶段、方案改变、遇到失败或结果未知时保存完整检查点。
4. **交接与结案**：保存下一步和未结操作；完成或放弃任务时提交对应终态检查点。

可以直接这样要求 Agent：

> 使用 Retenixa 恢复这个项目的相关任务。先检查保存时的依据和当前变化，再继续工作；每完成一个可核对阶段保存检查点。

### 常用工具

工具在客户端中可能带有服务名前缀，以下列出原始名称。参数以实际工具 schema 为准，也可通过 contract_get 查询当前能力。

| 用途 | 工具 |
| --- | --- |
| 打开项目、恢复任务 | context_open、context_resume |
| 列出、创建、认领、释放任务 | task_list、task_create、task_claim、task_release |
| 保存进度或结案 | checkpoint_save |
| 检索与精确读取 | memory_search、memory_get |
| 提交和修订知识 | knowledge_write |
| 确认写入结果、诊断服务 | request_get、system_doctor |

DSH 适配插件额外提供 pending_requests 和 retry_request，用于查看、确认或按原始载荷重投结果未知的请求。明确的版本冲突、归属冲突或引用错误会结案，不会继续占用待确认列表或阻止切换项目；原请求重试返回原错误。

### 检索与标签

`memory_search` 把输入当作普通文本。多词查询过滤常见中英文停用词和代码关键字，优先匹配全部剩余词，再用部分命中补齐结果和后续页。精确 ID 或 key 直达；同一匹配层内，路径精确匹配优先于目录前缀匹配，两者优先于不匹配路径。文本用 BM25 排序，key、条件、检索标签和正文的列权重依次降低。`scope.path_prefix` 仍限制检索范围，不匹配的路径不会因排序进入结果。

短词或没有 FTS 命中的查询走字面扫描：单次调用跨 500 行窗口查找，累计最多检查 5,000 条符合范围的记录。只有触及扫描上限且还有记录时，空结果才会带 `has_more` 和 `scan_limit_reached`；此时可用游标续查，或加路径和条件缩小范围。

同一任务在多个检查点重复记录的相同失败或未知尝试，在工作区和相关路径相同时只占一个检索结果。精确查询检查点 ID 时优先返回该检查点对应的尝试。原始快照和引用保留，仍可按检查点 ID 精确读取。

`knowledge_write` 的 `propose` 和 `revise` 可传 `search_tags`，最多 8 个、每个不超过 64 字符，例如 `["反复跳回登录", "session loop"]`。用标签补充正文没有出现的症状或同义词。标签随版本保存，只用于检索；修订时省略字段沿用旧标签，传 `[]` 清除。

### 恢复视图与预算

`context_resume` 的 `view` 可选 `compact`、`standard`（默认）或 `full`：

- `compact`：优先读取任务骨架、当前变化和未结操作；略去相关候选、固定正文和历史尝试/验证。
- `standard`：首屏最多补充两条相关知识摘要；固定正文按预算返回，放不下一页时可分页。
- `full`：首屏最多补充五条相关知识摘要；与 `standard` 一样受预算约束，仍可能裁剪。按检查点 ID 调 `memory_get` 可取原始快照和固定引用。

相关候选依据观测文件路径、固定知识的路径范围和阻塞项检索。每条摘要带 ID、revision、key、命中原因、适用性和最多 100 字摘录。候选不会自动纳入固定依据，正文按 ID 和 revision 调 `memory_get` 读取。

恢复和检索的 `byte_budget` 只计一份 UTF-8 JSON 载荷（MCP `structuredContent`），不计兼容镜像。恢复预算不足时依次省略相关候选、固定正文和历史细节，必要时再省略其他非核心元数据。检查 `omitted_sections`；仅当必要载荷仍超预算才返回 `BUDGET_TOO_SMALL`，可按 `min_required_bytes` 调整，或查看 `largest_fields` 中的字节数及占比。有 `next_cursor` 时沿用同一 `view` 翻页；如果 `fixed_reference_bodies` 被标记省略但没有游标，按固定引用的 ID 和版本用 `memory_get` 补读。

保存与恢复时，服务对相关文件流式计算 SHA-256，每个文件最多 32 MiB，每次观测累计最多 128 MiB。超预算的文件指纹标为 `unknown`；大体积构建产物不宜纳入相关路径。

### 跨会话与跨客户端交接

普通 MCP 新会话通常对应独立工作流；DSH 适配器可在同一可信宿主会话恢复时保留工作流身份。独立会话、fork 或另一个客户端不会仅因打开同一项目而获得已有任务的写权。

- **原所有者可操作**：先保存交接检查点，再用 `task_release` 释放非终态任务。接手者读取最新 revision、epoch 并调用 `task_claim`。
- **原普通 stdio 已退出**：Windows 和 Linux 上，新普通 stdio 可用当前 `expected_revision` 和 `expected_epoch` 认领，前提是本机确认原进程身份已终止。原进程仍在、进程身份未知、升级前的普通工作流以及 DSH 持久工作流均不会按退出规则接管。
- **需人工转交**：操作者可用短参数预览并在终端输入 `yes`；工具查出当前 revision、epoch，生成请求 ID，确认后按预览版本提交。脚本仍使用请求文件和显式确认。

~~~bash
retenixa.exe transfer --data-dir D:/Data/Retenixa --project PROJECT_ID --task TASK_ID --to WORKFLOW_ID --reason "交接后续工作"
retenixa.exe transfer --data-dir D:/Data/Retenixa --request-file D:/Data/transfer.json --confirm-transfer
~~~

管理请求文件包含 project_id、task_id、target_workflow_id、expected_revision、expected_epoch、request_id 和 reason。ID 与版本取自实际工具结果；一次逻辑转交保留同一 request_id。释放和转交都会提升 revision、epoch；旧所有者继续更新时，`OWNERSHIP_CHANGED` 提供 `task_id`、`current_revision`、`current_epoch` 和 `owner_workflow_id`。

**恢复包是工作线索，不是执行授权。** 对仍在运行或结果未知的命令、发布、远程任务，应先到原宿主核查；Retenixa 的请求去重不覆盖外部操作。

## 数据与维护

默认数据目录为 Windows 的 %LOCALAPPDATA%/Retenixa，也可用 --data-dir 指定绝对路径。程序启动 serve 时初始化数据库；数据不写入项目仓库。使用本机磁盘保存数据库，不把活动数据库当作普通文件通过同步盘或网络共享并发使用。

以下示例假定 retenixa.exe 已在 PATH 中；否则替换为实际可执行文件路径。

~~~bash
# 查看版本与完整命令列表
retenixa.exe --version
retenixa.exe --help

# 检查数据库与运行环境
retenixa.exe doctor --data-dir D:/Data/Retenixa

# 一致性备份：父目录须存在，输出文件必须尚不存在
retenixa.exe backup --data-dir D:/Data/Retenixa --output D:/Backups/retenixa-backup.sqlite3

# 恢复会覆盖当前数据库：先停止所有使用该数据目录的 Retenixa 进程
retenixa.exe restore --data-dir D:/Data/Retenixa --input D:/Backups/retenixa-backup.sqlite3

# 重建派生检索索引
retenixa.exe reindex --data-dir D:/Data/Retenixa

# 导出当前工具与数据契约的 JSON schema
retenixa.exe schema
~~~

备份可以在服务运行时执行。恢复要求独占维护访问，覆盖前会保存现有数据库备份。

### 普通目录初始化 Git 后的项目身份

核心 0.1.1 会把同一目录原有的项目身份升级为 Git 公共目录身份，保留项目、工作区和历史 ID；标准 `.git` 布局下，即使先从 linked worktree 接入，也会沿用主目录原有项目。新增或修改 Git 远程地址不参与身份判定。若 Git 身份已经属于另一份项目记忆，服务会返回 `PROJECT_SCOPE_MISMATCH` 和两边的项目 ID，需要显式整合。

`project-merge` 只处理同一标准 Git 主目录先登记为普通目录、后来又登记为 Git 项目的情形。请求文件使用绝对路径，内容如下（项目 ID 从错误详情中取得）：

~~~json
{
  "workspace_path": "D:/Projects/example",
  "source_project_id": "旧目录项目 ID",
  "target_project_id": "Git 项目 ID"
}
~~~

~~~bash
# 只读预演：不升级 schema、不改变项目记录
retenixa.exe project-merge --data-dir D:/Data/Retenixa --request-file D:/Requests/project-merge.json

# 先停止所有使用该数据目录的 Retenixa 进程，再显式执行整合
retenixa.exe project-merge --data-dir D:/Data/Retenixa --request-file D:/Requests/project-merge.json --confirm-project-merge
~~~

预演返回迁移行数、待保留的宿主身份、唯一键冲突和 `plan_hash`。将该哈希写入请求文件的可选字段 `expected_plan_hash`，执行时可核对预演后数据库是否变化。真实执行会先创建 `pre-project-merge-*.sqlite3` 一致性备份，再在一个事务中升级 schema 并整合记录；任一步失败都会回滚数据库事务。任务、检查点、知识和证据的 ID、版本、正文与固定引用保持不变，旧请求保留原哈希命名空间。同一宿主的不同历史工作流分别保留，不自动转交任务；其他唯一键冲突会拒绝执行。

整合后重新连接客户端，主目录与 worktree 可访问同一份项目记忆。升级到 schema 6 前，请停止指向同一数据目录的旧核心进程。新核心打开数据库时会从原始快照重建检索索引并生成尝试指纹，保留权威记录和请求回执。升级后使用支持 schema 6 的核心，旧检索游标需从首屏重查。DSH 可继续使用初始化 Git 前该会话已有的绑定与待确认请求。

DSH 插件另有需要随数据目录一起考虑的状态：绑定能力文件位于数据目录下的 host-bindings/dsh/，待确认请求的完整载荷记录在宿主持久存储中。宿主状态由 retenixa（绑定和请求身份索引）与 retenixa_requests（逐请求记录）两个域组成，备份宿主状态时应一起保存。SQLite 数据库备份不包含这两部分，只复制整个数据目录才会一并带上绑定文件。恢复较旧的数据库后，数据目录中的绑定文件仍记录着原代次，若高于数据库中的代次，附着会以 STALE_ATTACHMENT 停止：此时按原绑定恢复，或走显式转交，不手工改动代次。

不要把密码、访问令牌、密钥或恢复码写进记忆。记录必要的配置位置和脱敏现象即可。

## 当前范围

Retenixa 当前适合本机、同一可信用户下的项目记忆与任务接续。它不锁定源代码文件；并行改代码仍需要 worktree 隔离或其他协调方式。

批量导入、通用导出、受控清除、远程同步和 HTTP 服务尚未提供。DSH 插件支持通过原生工具、PTC 或嵌套调用写入，每个请求在发送前持久保存；结果未知时使用 pending_requests/retry_request 核查原请求。插件没有通用的压缩前自动保存保证，仍需要显式阶段检查点。

## 开发与验证

依赖和工具链分别由 [Cargo.toml](Cargo.toml)、[Cargo.lock](Cargo.lock) 和 [rust-toolchain.toml](rust-toolchain.toml) 固定。开发验证在仓库根目录执行：

~~~bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
node scripts/verify-core.mjs .
~~~

Rust 集成测试需要支持 git worktree add --orphan 的 Git。跨进程验收还需要 Node.js，并使用 target 下的独立合成目录；它检查真实 stdio 进程的任务认领、转交和备份恢复。

DSH 隔离验收的宿主依赖与调用方式见 [适配插件说明](adapters/dsh/README.md)。
