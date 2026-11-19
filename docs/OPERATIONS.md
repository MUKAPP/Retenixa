# 运维、迁移与发布

本文涵盖 Retenixa 的数据维护、普通目录转 Git 整合（`project-merge`）、Schema 升级、开发验证以及发版流程。快速上手见 [README](../README.md)。

## 数据目录与日常维护

默认数据目录为 Windows 的 `%LOCALAPPDATA%/Retenixa`，可通过 `--data-dir` 指定绝对路径。服务启动 `serve` 时自动初始化数据库，数据不写入项目仓库。

- **存储要求**：请将数据库保存在本机磁盘，切勿将活动数据库作为普通文件放在网盘同步目录或网络共享中并发使用。
- **敏感信息**：不要将密码、访问令牌、密钥或恢复码写进记忆，仅记录必要的配置位置和脱敏现象。

以下命令示例假设 `retenixa.exe` 已在 `PATH` 中，否则替换为实际可执行文件路径：

```bash
# 查看版本与完整命令列表
retenixa.exe --version
retenixa.exe --help

# 检查数据库引擎、schema、完整性与派生索引状态
retenixa.exe doctor --data-dir D:/Data/Retenixa

# 在线一致性备份：父目录须存在，输出文件必须尚不存在（可在服务运行时执行）
retenixa.exe backup --data-dir D:/Data/Retenixa --output D:/Backups/retenixa-backup.sqlite3

# 离线恢复：要求独占维护访问，覆盖前会自动备份现有数据库；执行前先停止所有使用该数据目录的进程
retenixa.exe restore --data-dir D:/Data/Retenixa --input D:/Backups/retenixa-backup.sqlite3

# 重建派生检索索引
retenixa.exe reindex --data-dir D:/Data/Retenixa

# 导出当前工具与数据契约的 JSON Schema
retenixa.exe schema
```

## 普通目录初始化 Git 与项目整合（`project-merge`）

核心 `0.1.1` 会将同一目录原有的项目身份升级为 Git 公共目录身份，保留项目、工作区和历史 ID（标准 `.git` 布局下，即使先从 linked worktree 接入，也会沿用主目录原有项目；新增或修改 Git remote 不参与身份判定）。

若 Git 身份已属于另一份项目记忆，服务会返回 `PROJECT_SCOPE_MISMATCH` 及两侧项目 ID，此时需显式执行 `project-merge`（仅处理同一标准 Git 主目录先登记为普通目录、后又登记为 Git 项目的情形）：

### 1. 准备请求文件

使用绝对路径，项目 ID 从错误详情中取得：

```json
{
  "workspace_path": "D:/Projects/example",
  "source_project_id": "旧目录项目 ID",
  "target_project_id": "Git 项目 ID"
}
```

### 2. 预演并执行整合

```bash
# 只读预演：不升级 schema、不改变项目记录
retenixa.exe project-merge --data-dir D:/Data/Retenixa --request-file D:/Requests/project-merge.json

# 先停止所有使用该数据目录的 Retenixa 进程，再显式执行整合
retenixa.exe project-merge --data-dir D:/Data/Retenixa --request-file D:/Requests/project-merge.json --confirm-project-merge
```

- **预演校验**：预演返回迁移行数、待保留的宿主身份、唯一键冲突和 `plan_hash`。可将 `plan_hash` 写入请求文件的可选字段 `expected_plan_hash`，执行时核对预演后数据库是否变化。
- **事务保证**：正式执行前会自动创建 `pre-project-merge-*.sqlite3` 一致性备份，随后在单事务中升级 schema 并整合记录；任一步失败即回滚。
- **记录保留**：任务、检查点、知识和证据的 ID、版本、正文与固定引用保持不变，旧请求保留原哈希命名空间；同一宿主的不同历史工作流分别保留，不自动转交任务；其他唯一键冲突会拒绝执行。
- **生效方式**：整合后重新连接客户端，主目录与 worktree 即可访问同一份项目记忆。

## Schema 6 升级与 DSH 外部状态

- **升级到 Schema 6**：升级前请停止指向同一数据目录的旧版核心进程。新版打开数据库时会从原始快照重建检索索引并生成尝试指纹，保留权威记录与请求回执。升级后旧检索游标需从首屏重查，DSH 可继续使用初始化 Git 前该会话已有的绑定与待确认请求。
- **DSH 状态备份边界**：SQLite 数据库备份**不包含**以下两部分，备份或迁移 DSH 环境时须一并考虑：
  1. 数据目录下的绑定能力文件：`host-bindings/dsh/`（完整复制整个数据目录才会包含）。
  2. 宿主持久存储中的两个域：`retenixa`（绑定和请求身份索引）与 `retenixa_requests`（逐请求记录，含待确认请求的完整载荷）。
- **`STALE_ATTACHMENT` 恢复**：若恢复了较旧的数据库，数据目录中绑定文件的代次可能高于数据库代次，附着会以 `STALE_ATTACHMENT` 停止。此时应按原绑定恢复或执行显式转交，**切勿手工修改代次**。

## 开发与验证

依赖与工具链分别由 [Cargo.toml](../Cargo.toml)、[Cargo.lock](../Cargo.lock) 和 [rust-toolchain.toml](../rust-toolchain.toml) 固定。在仓库根目录执行：

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
node scripts/verify-core.mjs .
```

- Rust 集成测试需要支持 `git worktree add --orphan` 的 Git。
- 跨进程验收（`scripts/verify-core.mjs`）需要 Node.js，并在 `target` 下的独立合成目录中验证真实 stdio 进程的任务认领、转交与备份恢复。
- DSH 隔离验收与交接验收见 [DSH 适配插件文档](../adapters/dsh/README.md)。

## 发布流程

[发布工作流](../.github/workflows/release.yml) 通过推送稳定版本标签 `vX.Y.Z` 触发：

1. 更新 [adapters/dsh/package.json](../adapters/dsh/package.json) 中的插件版本；仅在需要发布新核心时同步更新 [Cargo.toml](../Cargo.toml) 和 [Cargo.lock](../Cargo.lock)（若需通过此流程分发新核心，也须更新插件版本并打新标签）。
2. 将变更合入 `main`，为该提交创建并推送与插件版本完全一致的 `vX.Y.Z` 标签（只改版本或推送 `main` 不会触发发版）。
3. CI 测试并构建 Windows x64 核心（生成带核心版本号的 ZIP），再打包 DSH 插件 `.tgz`。GitHub Release 上传两个文件，npm 仅发布同一份 `.tgz`；两端独立发布，可在 Actions 中分别查看结果。

### npm Trusted Publishing 首次接入

1. 获取 npm `retenixa` 组织的发布权限。包尚未创建时，先由有权限的账号手动发布一次实际插件包（可直接使用首个 GitHub Release 中的 `.tgz`）：
   ```bash
   npm publish ./retenixa-dsh-adapter-X.Y.Z.tgz --access public --ignore-scripts
   ```
   建立信任前，该次 Actions 的 npm 发布任务会失败，不影响 GitHub Release 附件。
2. 确认发布工作流已合入远端后，在 npm 包的 **Settings → Trusted publishing** 中添加 GitHub Actions：用户 `MUKAPP`、仓库 `Retenixa`、工作流文件 `release.yml`、环境留空，并允许 `npm publish`；也可使用 npm `>= 11.15.0` 运行：
   ```bash
   npm trust github @retenixa/dsh-adapter --repo MUKAPP/Retenixa --file release.yml --allow-publish
   ```
   并在 npm 页面完成账号验证。
3. 建立信任后，在 npm 规定的两天内推送**下一个尚未发布的插件版本**标签完成首次 OIDC 发布并绑定仓库。已手动发布的版本无需重跑 npm 任务，后续自动发布不再需要 `NPM_TOKEN`。详见 [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers/)。
