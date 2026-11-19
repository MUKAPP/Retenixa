# Retenixa DSH 适配插件

版本 0.1.9；声明的 DSH 兼容范围为 `>=0.2.2-rc.1 <0.3.0-0`，不含 0.3.0 的预发布版。已通过验收的版本为 0.2.2-rc.1、0.2.2-rc.2 与 0.2.2-rc.3。本版目录转 Git 的身份迁移和项目整合需要 Retenixa 核心 0.1.1。核心 Rust MCP 服务独立运行，插件提供持久工作流绑定和插件详情页设置。

## 安装与设置

在 DSH 插件管理器中安装本地包目录或打包后的 tgz，可使用绝对路径或 file: 前缀。本包仍为 private，不提供 npm 注册表发布地址。

在仓库根目录打包（目标目录须先存在，不执行安装脚本）：

~~~bash
npm pack ./adapters/dsh --ignore-scripts --pack-destination ./target/packages
~~~

安装后，在插件详情页说明下方、包含的组件上方填写设置并保存：

| 设置 | 内容 |
| --- | --- |
| Retenixa 可执行文件 | retenixa.exe 的绝对路径；本包不内嵌核心二进制，必填 |
| 共享数据目录 | 默认 %LOCALAPPDATA%/Retenixa，与核心服务不带 --data-dir 时一致；需要与其他客户端共享记忆时改成同一路径 |

可执行文件未填时组件保持启用，便于直接在表单补齐；填好后连接核心服务。数据目录取自安装宿主进程的 LOCALAPPDATA，读取不到时该字段留空，需要手工填写。旧版安装若停用了组件，请先启用。保存配置会重新连接已打开的会话，持久绑定和原请求记录保留；其他页面并发修改时会拒绝覆盖，保留当前输入供重新载入。

项目目录来自 DSH 的 workingDirectory 服务。各会话独立选择目录，切换项目或 worktree 后，下一次工具调用连接对应目录；同一 Git 仓库的 worktree 共享项目身份并使用各自的工作区观测。普通目录初始化 Git 后沿用原项目，以及该会话原有的绑定能力和待确认请求。若旧版已为主目录和 worktree 分别建立项目，先按仓库维护说明使用 project-merge 预演并显式整合；插件会显示具体的源项目、目标项目与错误原因。旧配置中的 workspacePath 不再用于插件绑定。原目录有结果未知的请求时，切换会被拒绝：先返回原目录，通过 pending_requests/retry_request 核查，再切换。

也可在当前 profile 的用户补丁中配置。config 是整项替换，只写要改的字段即可；省略 dataDir 时使用 %LOCALAPPDATA%/Retenixa：

~~~yaml
- id: retenixa-dsh-adapter
  disabled: false
  config:
    command: 'D:/Tools/Retenixa/retenixa.exe'
    dataDir: 'D:/Data/Retenixa'
~~~

适配器自行启动 Retenixa stdio 进程，同一作用域无需再单独配置一份普通 Retenixa MCP 服务。

## Agent 使用规则

将 [AGENTS.md 规则模板](../../docs/AGENT_RULES.md) 的通用规则和 DSH 补充规则加入目标项目的代理指令。插件提供工具和协议提示；任务阶段保存、验证归因与结案由 Agent 按规则执行。

核心公布 `task_release` 时，适配器按普通写入流程代管其请求 ID。主动交接先保存阶段检查点，再释放任务。仅关闭 DSH 或其 stdio 子进程不会释放任务，原工作流仍可在可信宿主重启后恢复。

## 依赖与能力

宿主需提供 agents、tools/native executor、workingDirectory、sessionProjections、systemPrompt、持久 session 及 storageDomain 后端。这些官方服务作为 peer 依赖由宿主提供，MCP 客户端和 zod 随包安装。

插件自身启动 Retenixa stdio 进程并管理每个会话的工作流身份与请求记录。核心服务不可用时，工具调用直接报错，不会退回本地缓存或写入其他位置。

## 验证

[隔离 SDK 验收](../../scripts/verify-dsh.mjs) 使用真实 DSH JSONL 持久 session 和 storageDomain，验证重启同 workflow、generation 推进、服务提交后响应丢失按原 ID 确认，以及未知外部操作保留。

该脚本是模块入口，dshRoot 指向待验收版本的 SDK 根目录，其 node_modules 中需具备下述宿主模块。使用桌面版 app.asar 内的模块时，在仓库根目录用 DSH 自带的运行时执行：

~~~bash
ELECTRON_RUN_AS_NODE=1 "D:/APP/DeepSeek Harness/DeepSeek Harness.exe" --input-type=module -e "
import { verifyDsh } from './scripts/verify-dsh.mjs';
const report = await verifyDsh({
  dshRoot: 'D:/APP/DeepSeek Harness/resources/app.asar/dsh',
  projectRoot: process.cwd(),
});
console.log(report.status, report.root);
"
~~~

也可用 Node.js 执行模块入口，并将 dshRoot 指向隔离安装的同版本官方 npm SDK；0.2.2-rc.2 与 0.2.2-rc.3 已使用 Node.js 24.22.1 通过验收。隔离宿主需提供 dsh-session、dsh-session-projection、dsh-agent、dsh-llm、dsh-system-prompt、dsh-tools、dsh-tool-executor-native、dsh-session-persistence-jsonl、dsh-storage、dsh-storage-json、dsh-storage-domain、dsh-agent-loop 及本包声明的依赖。

需要先 `cargo build --locked`，验收固定使用 target/debug/retenixa.exe。验收使用嵌入式显式目录（config.workspacePath），插件在真实 GUI 中使用的自动目录来自宿主 workingDirectory 服务。通过时 report 给出 status: passed、dshVersion、同 session 的两次 generation 和保留的未知外部操作。

[交接验收](../../scripts/verify-handoff.mjs) 导出 verifyHandoff，在上面基础上追加 Codex 客户端；它需要本机 codex 可执行文件路径，并以同一方式调用。

打包产物和隔离验收日志保存在 target，均不进入版本控制。

## 实现说明

本节记录源码结构，改动插件时可据此后跳转，日常安装配置无需阅读。

[插件入口](plugin.js) 使用原生 Config 的 volatile 字段和 settings 表单接口，并监听 loader/volatile-update 响应配置值变化；[浏览器入口](client.js) 注册公开 plugins.bundle.config 插槽，在与其他插件相同的位置显示设置，写入带版本检查的 configForms。设置不单独存一份文件。插件空配置可挂载，保存后重建适配器子作用域。

[适配逻辑](index.js) 按各 Agent 作用域注册工具；每次排队调用前检查该会话的当前目录。模型工具隐藏 context_id 和变更 request_id；checkpoint_save 由 related_paths 及可选 base_commit 构造未知现场字段，服务另行观测，Agent 提供语义进展。私有绑定能力不进入模型 schema 或日志。嵌入式 SDK 调用 install(ctx, config, hostRequire) 时仍可显式提供 workspacePath，以支持无工作目录服务的隔离宿主。

context_open 读取任务索引；pending_requests 列出待确认的原请求；retry_request 仅确认或重投已持久化的原载荷。STALE_ATTACHMENT 停止旧实例。写入支持原生、PTC 和嵌套调用：能唯一关联持久根工具事件时沿用事件索引，其他调用各自生成独立索引。请求 ID、完整载荷和待确认身份索引全部持久保存成功后，才发送到核心服务。

版本冲突、归属冲突或引用不可用等明确业务拒绝会结案并移出待确认列表；重试同一请求返回原错误，不再次发送。传输、存储及过期附着错误仍保留待确认请求。已确认提交的请求释放原载荷，后续只查询原回执；回执或请求记录缺失时停止，不把旧请求当成新操作。

宿主持久存储分为 retenixa（轻量绑定、请求身份索引和待确认列表，版本 2）与 retenixa_requests（逐请求记录）。请求确认后不再保留完整请求正文和重复回执。升级时先逐条保存旧请求，再移除该绑定中的旧正文；中断重入保留已保存的新记录。若宿主为 Retenixa 单独配置 storageDomain 的 routes，请为这两个域指定同一后端。

绑定能力文件写在数据目录下的 host-bindings/dsh/，待确认请求的完整载荷存在宿主持久域，两者都不在 SQLite 备份中。
