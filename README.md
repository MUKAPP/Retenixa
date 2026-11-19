# Retenixa

**基于本地 SQLite 的 MCP 项目记忆服务，面向多客户端与多 Git worktree 提供可靠的任务接续、版本化知识检索与并发写入协调。**

> 验证平台：**Windows x86_64** ｜ 核心版本：`0.1.1` ｜ DSH 适配插件：`0.1.9` ｜ 协议：[MIT](LICENSE)

Retenixa 将任务进度、项目知识、失败经验和交接依据存入本机 SQLite，让不同会话、不同 MCP 客户端共享同一份项目上下文，减少重复调查并保留可追溯的工作依据。恢复任务时，同时返回**保存时的上下文**与**当前现场变化**（知识版本变更、相关文件变动与未结操作），并通过版本与归属校验防止多 Agent 并发覆盖。

## 核心能力

- **接续任务**：保存完整检查点，记录目标、进展、尝试结果、验证状态与下一步计划。
- **共享项目知识**：按项目、worktree、路径与适用条件检索，支持中英文及代码标识符搜索。
- **固定历史依据**：检查点绑定知识与证据的精确版本；知识后续被修订、争议或撤回时，恢复阶段会明确提示。
- **记录失败尝试**：检索已验证失败或结果未知的方案及其触发条件，避免重复走弯路。
- **协调并发写入**：任务归属明确，旧版本写入会被拒绝；结果未知的请求可按原 `request_id` 确认。

所有数据保存在本机，无需云账户、API Key、向量数据库或外部模型服务。同一 Git 仓库的多个 worktree 自动共享项目身份并保留各自工作区信息；非 Git 目录同样可用。

## 快速开始

### 1. 获取核心程序

从 [GitHub Releases](https://github.com/MUKAPP/Retenixa/releases) 下载 Windows 核心压缩包，解压后即可运行 `retenixa.exe`（内置 SQLite，无需额外安装）。

如需从源码构建（需 Rust `1.95.0`、MSVC C/C++ Build Tools 与 Git），在仓库根目录执行：

```bash
cargo build --release --locked
./target/release/retenixa.exe doctor
```

生成的二进制位于 `target/release/retenixa.exe`，运行 `retenixa.exe --version` 可确认核心版本。

### 2. 选择接入方式

| 维度 | 普通 MCP 客户端（stdio） | DSH 适配插件（`@retenixa/dsh-adapter`） |
| --- | --- | --- |
| 适用场景 | 各类支持 stdio 的通用 MCP 客户端 | DeepSeek Harness（DSH）用户 |
| 项目路径 | 在 `context_open` 中显式传入项目绝对路径 | 自动跟随各会话的 `workingDirectory`，无需配置 `workspacePath` |
| 工作流身份 | 新会话使用 `fresh` 模式 | 自动维护持久工作流绑定，合法重启可续接原工作流 |
| 未知请求恢复 | 调用方自行保留 `request_id` 并调用 `request_get` | 自动持久化请求载荷，提供 `pending_requests` / `retry_request` |

> 每个客户端可运行独立进程，无需另开终端常驻 `serve`；**需要共享记忆的客户端指向同一个 `--data-dir` 即可**。同一作用域已启用 DSH 适配插件时，无需再重复配置普通 MCP 服务。

#### 方式 A：普通 MCP 客户端

在客户端中添加 stdio MCP 服务，`command` 填 `retenixa.exe` 绝对路径，`args` 指定 `serve` 与数据目录（以下为 `mcpServers` 示例，外层结构以实际客户端为准）：

```json
{
  "mcpServers": {
    "retenixa": {
      "command": "D:/Tools/Retenixa/retenixa.exe",
      "args": ["serve", "--data-dir", "D:/Data/Retenixa"]
    }
  }
}
```

连接后让 Agent 调用 `context_open` 并传入项目绝对路径（`fresh` 模式）。读取旧任务无需接管；若需继续写入已有所有者的任务，须按交接流程处理，详见 [核心机制与交接](docs/MECHANICS.md)。

#### 方式 B：DSH 适配插件

通过 DSH 原生 CLI 或界面安装（详见 [DSH 适配插件文档](adapters/dsh/README.md)）：

- **DSH CLI**（桌面版操作前须先完全退出桌面应用，Web 版将 `desktop` 改为 `web` 或对应 profile 名）：
  ```bash
  # 从 GitHub Release 下载的 .tgz 安装
  dsh plugin --profile desktop add ./retenixa-dsh-adapter-0.1.9.tgz

  # 或从本地源码目录安装
  dsh plugin --profile desktop add ./adapters/dsh
  ```
- **DSH GUI**：在左侧栏打开「**插件**」→ 点击「**添加插件**」，填入本地 `.tgz` 路径或本地包目录路径安装并启用。

安装后，在插件详情页填写以下设置并保存（旧版若停用了组件，请先启用）：

| 字段 | 说明 |
| --- | --- |
| `command` | `retenixa.exe` 的绝对路径（必填） |
| `dataDir` | 默认 `%LOCALAPPDATA%/Retenixa`（与核心不带 `--data-dir` 一致），与其他客户端共享记忆时填同一路径 |

## 让 Agent 使用项目记忆

完成接入后，将 [AGENTS.md 规则模板](docs/AGENT_RULES.md) 中的**通用规则**与对应接入方式的**补充规则**复制到目标项目的 `AGENTS.md`（或客户端实际加载的指令文件）。

### 日常工作流

纯只读问答和无需接续的一句话小改动可直接跳过建任务与存检查点；需要追踪或交接时按以下步骤执行：

1. **开始或恢复**：调用 `context_open` 打开项目上下文，`task_list` 查看相关任务，`context_resume` 读取恢复包并核对现场。
2. **工作前检索**：按任务、路径与条件检索知识或失败尝试，必要时用 `memory_get` 读取完整历史版本。
3. **阶段保存**：完成可核对阶段、变更方案、遇到失败或出现结果未知的操作时，调用 `checkpoint_save` 保存完整检查点。
4. **交接与结案**：保存下一步计划与未结操作；完成或放弃任务时提交对应终态检查点。

可以直接这样指示 Agent：

> 使用 Retenixa 恢复这个项目的相关任务。先检查保存时的依据和当前变化，再继续工作；每完成一个可核对阶段保存检查点。

### 工具总览

工具在客户端中可能带有服务名前缀，完整参数以工具 Schema 或 `contract_get` 为准。

| 类别 | 工具 | 说明 |
| --- | --- | --- |
| 上下文与恢复 | `context_open`、`context_resume` | 打开项目/工作流、只读恢复检查点与现场变化 |
| 任务管理 | `task_list`、`task_create`、`task_claim`、`task_release` | 列出、创建、认领与释放任务 |
| 检查点 | `checkpoint_save` | 保存不可变阶段快照、固定引用或结案终态 |
| 检索与读取 | `memory_search`、`memory_get` | 检索知识与失败尝试、按 ID/版本精确读取原文 |
| 知识管理 | `knowledge_write` | 提交（`propose`）和修订（`revise`）项目知识 |
| 确认与诊断 | `request_get`、`system_doctor`、`contract_get` | 确认历史写入结果、检查数据库状态、查询工具契约 |
| DSH 插件扩展 | `pending_requests`、`retry_request` | 查看、确认或按原载荷重投结果未知的请求 |

## 当前边界

- 面向本机、同一可信用户下的项目记忆与任务接续。
- 不锁定源代码文件；并行修改代码仍需借助 Git worktree 或其他隔离方式。
- 批量导入、通用导出、受控清除、远程同步与 HTTP 服务尚未提供。
- DSH 插件支持原生工具、PTC 与嵌套调用写入（发送前持久化请求），但不提供通用的上下文压缩前自动保存保证，仍需显式保存阶段检查点。
- **安全边界**：切勿将密码、访问令牌、密钥或恢复码写入记忆；恢复包仅是工作线索，不构成外部操作授权。

## 文档导航

| 文档 | 内容 |
| --- | --- |
| [AGENTS.md 规则模板](docs/AGENT_RULES.md) | 供 Agent 直接使用的通用规则与普通 MCP / DSH 接入补充 |
| [核心机制与交接](docs/MECHANICS.md) | 检索排序与标签、恢复视图与字节预算、跨会话/跨客户端交接与 `transfer` 命令 |
| [运维、迁移与发布](docs/OPERATIONS.md) | 数据备份与恢复、普通目录转 Git 与 `project-merge`、Schema 6 升级、开发验证与发版流程 |
| [DSH 适配插件](adapters/dsh/README.md) | DSH 原生 CLI / GUI 安装、配置项、目录绑定行为、隔离验收与开发者实现说明 |
