# Retenixa DSH 适配插件

DeepSeek Harness（DSH）的 Retenixa 适配插件，提供持久工作流绑定、请求持久化恢复以及插件详情页可视化设置。核心 Rust MCP 服务独立运行，本插件负责自动拉起与会话级托管。

| 项目 | 说明 |
| --- | --- |
| 插件包名与版本 | `@retenixa/dsh-adapter` `0.1.9` |
| DSH 兼容范围 | `>=0.2.2-rc.1 <0.3.0-0`（不含 `0.3.0` 预发布版） |
| 已验收 DSH 版本 | `0.2.2-rc.1`、`0.2.2-rc.2`、`0.2.2-rc.3` |
| 所需 Retenixa 核心版本 | `0.1.1`（目录转 Git 身份迁移与项目整合依赖此版本） |

## 安装与配置

### 1. 准备核心程序

本插件**不内嵌**核心可执行文件，请先从 [GitHub Releases](https://github.com/MUKAPP/Retenixa/releases) 下载 Windows 核心压缩包，或从仓库源码构建 `retenixa.exe`。

### 2. 通过 DSH 原生方式安装插件

DSH 支持通过原生 **CLI** 或 **GUI** 直接安装 `.tgz` 压缩包或本地包目录。

#### 方式一：DSH CLI 安装

> 桌面版与 Web 版使用相互隔离的 profile（桌面版为 `desktop`，Web 版通常为 `web`）。操作桌面版前须先启动过一次完成初始化，并在执行命令前**完全退出桌面应用**（Windows 请从系统托盘退出），避免进程锁冲突。

```bash
# 安装从 GitHub Releases 下载的 .tgz（Web 版将 desktop 替换为 web 或对应 profile 名）
dsh plugin --profile desktop add ./retenixa-dsh-adapter-0.1.9.tgz

# 或直接安装本地源码目录（支持绝对路径或 file: 前缀）
dsh plugin --profile desktop add ./adapters/dsh
```

如需卸载，执行：

```bash
dsh plugin --profile desktop remove @retenixa/dsh-adapter
```

如需自行从仓库源码打包 `.tgz`（目标目录须先存在，不执行安装脚本）：

```bash
npm pack ./adapters/dsh --ignore-scripts --pack-destination ./target/packages
```

#### 方式二：DSH GUI 安装

1. 打开 DeepSeek Harness 桌面版或 Web UI。
2. 点击左侧栏「**插件**」（或「**设置 → 插件**」），点击「**添加插件**」。
3. 填入本地 `.tgz` 绝对路径或本地包目录路径，点击安装。
4. 安装完成后按界面提示启用并重启生效。

### 3. 填写设置

在 DSH 插件详情页（说明下方、组件列表上方）填写并保存以下字段（若旧版安装停用了组件，请先启用）：

| 设置项 | 说明 |
| --- | --- |
| **Retenixa 可执行文件**（`command`） | `retenixa.exe` 的绝对路径（必填） |
| **共享数据目录**（`dataDir`） | 默认 `%LOCALAPPDATA%/Retenixa`（与核心不带 `--data-dir` 一致）；与其他客户端共享记忆时改为同一路径 |

- `command` 未填时组件保持启用，方便直接在表单补齐；填好后自动连接核心服务。
- `dataDir` 读取自安装宿主进程的 `LOCALAPPDATA`，若读取不到则留空，需手动填写。
- 保存配置会自动重连已打开的会话，同时保留持久绑定与原请求记录；若其他页面并发修改，将拒绝覆盖并保留当前输入供重新载入。

也可通过当前 profile 的用户补丁（`cordis.patch.yml`）配置（`config` 为整项替换，仅写需要覆盖的字段；省略 `dataDir` 时使用默认值）：

```yaml
- id: retenixa-dsh-adapter
  disabled: false
  config:
    command: 'D:/Tools/Retenixa/retenixa.exe'
    dataDir: 'D:/Data/Retenixa'
```

> 适配器自行管理 Retenixa stdio 子进程，同一作用域下无需再单独配置普通 Retenixa MCP 服务。

### 4. 目录绑定与切换行为

- **自动跟随工作目录**：项目目录取自 DSH 的 `workingDirectory` 服务（旧配置中的 `workspacePath` 已不再用于插件绑定）。各会话独立选择目录，切换项目或 worktree 后，下一次工具调用会自动切至对应目录；同一 Git 仓库的 worktree 共享项目身份并保留各自工作区观测。
- **普通目录初始化 Git**：初始化后沿用原项目及该会话原有的绑定能力与待确认请求。若旧版曾为主目录与 worktree 分别建过项目，请按 [运维、迁移与发布](../../docs/OPERATIONS.md) 中的 `project-merge` 流程预演并整合（插件报错会直接给出源项目、目标项目与原因）。
- **未确认请求保护**：原目录存在结果未知的请求时，切换目录会被拒绝；请先切回原目录，使用 `pending_requests` / `retry_request` 核查结案后再切换。

## Agent 使用规则

将 [AGENTS.md 规则模板](../../docs/AGENT_RULES.md) 中的**通用规则**与 **DSH 补充规则**加入目标项目的代理指令。插件仅负责工具接入与协议提示，任务阶段保存、验证归因与结案由 Agent 按规则执行。

- 核心暴露 `task_release` 时，适配器按普通写入流程代管其 `request_id`；主动交接前先保存阶段检查点，再释放任务。
- 仅关闭 DSH 或终止 stdio 子进程不会释放任务，原工作流可在可信宿主重启后继续恢复。

## 宿主依赖与运行边界

- **Peer 依赖**：宿主需提供 `agents`、`tools` / native executor、`workingDirectory`、`sessionProjections`、`systemPrompt`、持久 `session` 及 `storageDomain` 后端；`@modelcontextprotocol/client` 与 `zod` 随包安装。
- **故障策略**：核心服务不可用时，工具调用直接报错，不会退回本地缓存或静默写入其他位置。

## 验收与测试

验证前需在仓库根目录执行 `cargo build --locked`（验收脚本固定使用 `target/debug/retenixa.exe`）。打包产物与验收日志均写入 `target`，不纳入版本控制。

### 隔离 SDK 验收（`verify-dsh.mjs`）

[scripts/verify-dsh.mjs](../../scripts/verify-dsh.mjs) 基于真实 DSH JSONL 持久 session 与 `storageDomain`，验证重启续接同 workflow、`generation` 推进、服务提交后响应丢失按原 ID 确认以及未知外部操作保留。

使用桌面版 `app.asar` 内模块时，可在仓库根目录用 DSH 自带运行时执行：

```bash
ELECTRON_RUN_AS_NODE=1 "D:/APP/DeepSeek Harness/DeepSeek Harness.exe" --input-type=module -e "
import { verifyDsh } from './scripts/verify-dsh.mjs';
const report = await verifyDsh({
  dshRoot: 'D:/APP/DeepSeek Harness/resources/app.asar/dsh',
  projectRoot: process.cwd(),
});
console.log(report.status, report.root);
"
```

也可用 Node.js 直接运行模块入口，将 `dshRoot` 指向隔离安装的同版本官方 npm SDK（`0.2.2-rc.2` 与 `0.2.2-rc.3` 已在 Node.js `24.22.1` 下通过验收）。隔离宿主需提供以下模块：`dsh-session`、`dsh-session-projection`、`dsh-agent`、`dsh-llm`、`dsh-system-prompt`、`dsh-tools`、`dsh-tool-executor-native`、`dsh-session-persistence-jsonl`、`dsh-storage`、`dsh-storage-json`、`dsh-storage-domain`、`dsh-agent-loop` 及 `package.json` 声明的依赖。

> 注：验收使用嵌入式显式目录（`config.workspacePath`），真实 GUI 场景下则使用宿主 `workingDirectory` 服务。通过时 `report` 返回 `status: passed`、`dshVersion`、同 session 的两次 `generation` 及保留的未知外部操作。

### 跨客户端交接验收（`verify-handoff.mjs`）

[scripts/verify-handoff.mjs](../../scripts/verify-handoff.mjs) 导出 `verifyHandoff`，在 `verifyDsh` 基础上追加 Codex 客户端交接验收；需提供本机 `codex` 可执行文件路径，调用方式与 `verifyDsh` 一致。

## 实现说明（开发者参考）

日常安装与使用无需阅读本节。

### 源码结构

| 文件 | 职责 |
| --- | --- |
| [plugin.js](plugin.js) | 插件入口：定义原生 Config 的 `volatile` 字段与 `settings` 表单接口，监听 `loader/volatile-update` 响应配置变更；支持空配置挂载并在保存后重建适配器子作用域 |
| [client.js](client.js) | 浏览器端入口：注册公开 `plugins.bundle.config` 插槽，在统一位置渲染设置项，写入带版本检查的 `configForms`（不额外写独立配置文件） |
| [index.js](index.js) | 核心适配逻辑：按 Agent 作用域注册工具，排队调用前校验会话当前目录；嵌入模式调用 `install(ctx, config, hostRequire)` 时仍支持显式 `workspacePath` |

### 关键行为约定

- **字段代管与隐私边界**：宿主代填 `context_id`、变更 `request_id` 与 `snapshot.workspace`，拒绝调用方自行传入；私有绑定能力不进入模型 Schema 或日志。`checkpoint_save` 仅接收 `related_paths` 与可选 `base_commit`，现场观测由核心服务采集。
- **请求持久化与重试**：支持原生、PTC 与嵌套调用写入（能唯一关联持久根工具事件时沿用事件索引，其余生成独立索引）。仅在请求 ID、完整载荷与待确认身份索引全部落盘成功后才发往核心服务。`context_open` 读取任务索引，`pending_requests` 列出待确认原请求，`retry_request` 仅确认或重投已持久化载荷；遇到 `STALE_ATTACHMENT` 立即停止旧实例。
- **错误结案策略**：版本冲突、归属冲突或引用不可用等明确业务拒绝会结案并移出待确认列表，重试原请求直接返回原错误、不再重发；传输、存储及过期附着错误则保留待确认状态。已确认提交的请求释放原载荷，后续只查回执；回执或记录缺失时立即停止，不将旧请求当作新操作。
- **双域持久存储**：宿主持久存储分为 `retenixa`（轻量绑定、请求身份索引与待确认列表，v2）与 `retenixa_requests`（逐请求记录）。请求确认后不再保留完整正文与重复回执；版本升级时先逐条落盘旧请求再清理旧正文，支持中断重入。若宿主为 Retenixa 单独配置了 `storageDomain` 的 `routes`，请确保两个域指向同一后端。
- **备份边界**：绑定能力文件位于数据目录 `host-bindings/dsh/`，待确认请求完整载荷存于宿主持久域，两者均不包含在 SQLite 数据库备份中。