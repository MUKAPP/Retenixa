# 核心机制与交接

本文说明 Retenixa 的检索排序、恢复视图与字节预算控制，以及跨会话、跨客户端的任务交接规则。基础接入与工具总览见 [README](../README.md)。

## 检索与标签

- **查询解析与排序**：`memory_search` 将输入视为普通文本，不执行 FTS 表达式。多词查询过滤常见中英文停用词和代码关键字，优先匹配全部剩余词，再用部分命中补齐结果与后续页；精确 ID 或 `key` 直达。同匹配层内按**路径精确匹配 > 目录前缀匹配 > 不匹配路径**排序，文本采用 BM25 计分，列权重依次为 `key` > 条件 > `search_tags` > 正文。`scope.path_prefix` 为硬过滤条件，不匹配路径不会因排序进入结果。
- **字面扫描兜底**：短词或无 FTS 命中时退回字面扫描，单次调用跨 500 行窗口，累计最多检查 5,000 条符合范围的记录。仅在触及上限且仍有剩余记录时，空结果才携带 `has_more` 与 `scan_limit_reached`；此时可用游标续查，或通过路径与条件缩小范围。
- **失败尝试去重**：同一任务在多个检查点重复记录的相同失败或未知尝试，在工作区与相关路径相同时只占一条检索结果。精确查询检查点 ID 时优先返回该检查点对应尝试；原始快照和引用均保留，仍可按检查点 ID 调用 `memory_get` 精确读取。
- **检索标签（`search_tags`）**：`knowledge_write` 的 `propose` 和 `revise` 支持传入 `search_tags`，最多 8 个、每个不超过 64 字符（如 `["反复跳回登录", "session loop"]`），用于补充正文未出现的症状或同义词。标签随版本保存且仅用于检索；修订时省略该字段沿用旧标签，传 `[]` 清除。

## 恢复视图与字节预算

`context_resume` 的 `view` 支持三种模式：

| 视图 | 行为 |
| --- | --- |
| `compact` | 优先返回任务骨架、当前变化和未结操作；省略相关候选、固定正文与历史尝试/验证 |
| `standard`（默认） | 首屏最多附带 2 条相关知识摘要；固定正文按预算返回，放不下一页时可分页 |
| `full` | 首屏最多附带 5 条相关知识摘要；与 `standard` 一样受预算约束，仍可能裁剪 |

> 需要完整原始快照和固定引用时，可按检查点 ID 直接调用 `memory_get`。

- **相关候选**：依据观测文件路径、固定知识的路径范围与阻塞项检索。每条摘要含 ID、`revision`、`key`、命中原因、适用性和最多 100 字摘录。候选不会自动纳入固定依据，正文需按 ID 和 `revision` 调用 `memory_get` 读取。
- **单载荷预算与裁剪**：恢复与检索的 `byte_budget` 仅计一份 UTF-8 JSON 载荷（MCP `structuredContent`），不计兼容镜像。预算不足时依次省略相关候选、固定正文、历史细节及其他非核心元数据，并在 `omitted_sections` 中标明。仅当必要载荷仍超预算时返回 `BUDGET_TOO_SMALL`，可参考 `min_required_bytes` 调整，或查看 `largest_fields` 中的字节数及占比。
- **翻页与补读**：返回 `next_cursor` 时沿用同一 `view` 翻页；若 `fixed_reference_bodies` 被标记省略但无游标，按固定引用的 ID 和版本调用 `memory_get` 补读。
- **文件指纹预算**：保存与恢复时对相关文件流式计算 SHA-256，单文件上限 32 MiB，单次观测累计上限 128 MiB。超限文件的指纹标为 `unknown`，大体积构建产物不宜纳入相关路径。

## 跨会话与跨客户端交接

普通 MCP 新会话对应独立工作流；DSH 适配器可在同一可信宿主会话恢复时保留工作流身份。独立会话、fork 或其他客户端不会仅因打开同一项目而获得已有任务的写权。

### 三种交接路径

| 场景 | 操作方式 |
| --- | --- |
| **原所有者可操作** | 先保存交接检查点，再用 `task_release` 释放非终态任务；接手方读取最新 `revision`、`epoch` 并调用 `task_claim` |
| **原普通 stdio 进程已退出** | 在 Windows 和 Linux 上，若本机确认原进程身份已终止，新普通 stdio 可凭当前 `expected_revision` 与 `expected_epoch` 调用 `task_claim` 直接接管（原进程仍在、身份未知、升级前普通工作流及 DSH 持久工作流均不适用） |
| **需要人工管理转交** | 通过 CLI `transfer` 交互预览确认，或通过管理请求文件提交 |

### `transfer` 管理转交命令

```bash
# 交互式转交：自动查询当前 revision/epoch 并生成 request_id，在终端预览后输入 yes 确认
retenixa.exe transfer --data-dir D:/Data/Retenixa --project PROJECT_ID --task TASK_ID --to WORKFLOW_ID --reason "交接后续工作"

# 非交互脚本模式：传入管理请求 JSON 并显式确认
retenixa.exe transfer --data-dir D:/Data/Retenixa --request-file D:/Data/transfer.json --confirm-transfer
```

- **请求文件字段**：须包含 `project_id`、`task_id`、`target_workflow_id`、`expected_revision`、`expected_epoch`、`request_id` 和 `reason`。ID 与版本取自实际工具结果，一次逻辑转交保留同一 `request_id`。
- **并发保护**：释放和转交都会推进 `revision` 与 `epoch`。旧所有者继续更新时，`OWNERSHIP_CHANGED` 会返回 `task_id`、`current_revision`、`current_epoch` 和 `owner_workflow_id`。
- **DSH 适配器结案规则**：明确的版本冲突、归属冲突或引用错误会直接结案，不会继续占用待确认列表或阻止切换项目；重试原请求返回原错误。

> **安全边界**：恢复包是工作线索，不是执行授权。对仍在运行或结果未知的命令、发布或远程任务，必须先回原宿主核查；Retenixa 的请求去重不覆盖外部操作。