# Editor MCP server

`crates/prism-editor-mcp` 把编辑器数据层（`prism-editor-api`）暴露成 MCP 工具，
让 agent 直接改场景、跑游戏、抓图，不用手改 JSON 再重新 cook。

它**不链接** engine / render，也不开 Vulkan：读写走 `prism-editor-api` 的反射
校验，需要画面时用 `screenshot` 起一个子进程。

## 启动

```bash
cargo run -p prism-editor-mcp -- --project projects/fishing
```

项目根也可以用 `PRISMREV_PROJECT` 指定；都没给时从 cwd 向上找 `assets/`。
传输是 stdio，一行一条 JSON-RPC 消息。

在 `.mcp.json` 里挂上去（示例）：

```json
{
  "mcpServers": {
    "prisma-editor": {
      "command": "cargo",
      "args": [
        "run", "--quiet", "-p", "prism-editor-mcp", "--",
        "--project", "projects/fishing"
      ]
    }
  }
}
```

## 工具

| 工具 | 作用 |
|---|---|
| `documents_list` | 列出场景 / 预制体（项目相对路径） |
| `document_read` | 读完整文档（已迁移到当前格式，实体带稳定 `id`） |
| `document_write` | 校验后整体写回 |
| `schema_list` | 所有组件 + 嵌套结构体的字段、枚举变体、只读标记 |
| `entity_get` | 按 `id` 或名字取实体 |
| `field_set` | **反射写入**一个字段（`a.b` / `list[0].x` 都支持） |
| `component_add` / `component_remove` | 挂 / 摘组件（默认值来自 schema） |
| `game_run` / `game_stop` / `game_logs` | 起停游戏、读日志尾部 |
| `screenshot` | 起游戏 → 抓图 → 结束，返回 PNG 路径 |

### `field_set` 为什么安全

写入走 `prism_editor_api::reflect_edit::set_component_field`：
`from_object` 还原组件 → `Reflect::set_field` 写字段（`#[reflect(read_only)]`
被拒、类型经反射校验）→ `object_to_json` 还原作者 JSON。所以非法类型、未知
枚举变体、只读字段都不会落到磁盘上。

### `screenshot`

参数：`path`（必填）、`size`（`1920x1080`）、`scale`（窗口倍数）、`frame`
（默认 120）、`timeout_ms`、`command`、`env`。

它复用引擎的内置截图（`PRISMREV_SCREENSHOT*`，见 `AGENTS.md`），所以抓的是
**屏幕上那一帧**；缩放是最近邻，像素画放大不糊。

`command` 默认 `cargo run --release`，在项目根目录执行。想跳过 cargo 的启动
开销就直接给二进制路径，例如
`"command": "./target/release/fishing"`。

## 边界

- 只做数据 + 进程，不做渲染。想改渲染器行为要回引擎 crate。
- 不实现 resource / prompt / sampling；`tools/list` 之外只有 `ping`。
- 没有并发保护：同一个文档别同时开两个 agent 写（`document_write` 是整文件覆盖）。
