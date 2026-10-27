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

## 编辑器 UI 操作（CDP）

除了改数据，还能像浏览器 MCP 那样操作**编辑器界面本身**：列出按钮 / 文本 /
输入框，然后点击、改值、高亮。

实现走 CDP（Chrome DevTools Protocol）——`prism-editor-mcp` 连到 Tauri
（WebView2）的调试端口，在页面里跑 DOM 脚本。**不需要改编辑器代码**，也不用
重新构建前端；WebSocket 客户端是手写的（RFC 6455），没有新增依赖。

### 前提：带调试端口启动编辑器

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
cd editor
pnpm tauri dev
```

或者用 `scripts/editor-cdp.ps1`（等价，省得记环境变量）。

端口被占用时换一个，工具都接受 `port` 参数。

### 工具

| 工具 | 作用 |
|---|---|
| `ui_snapshot` | 返回 `{url, title, viewport, nodes[]}`；每个节点有 `id` / `role` / `label` / `bounds` / `value` / `disabled` |
| `ui_click` | `scrollIntoView` + 完整 `pointerdown → mousedown → pointerup → mouseup → click` 序列 |
| `ui_fill` | 用原生 setter 写值，再派发 `input` + `change`（Vue 的 `v-model` 能收到） |
| `ui_highlight` | 画一个洋红高亮框（默认 1500 ms，其余区域压暗） |
| `ui_eval` | 逃生口：直接跑 JS 并返回值 |
| `ui_screenshot` | `Page.captureScreenshot` → PNG |

### id 的稳定性

第一次快照时给元素打 `data-prism-ui="uN"`，之后一直复用。所以可以
`ui_snapshot` 一次、连续 `ui_click` / `ui_fill`；但**重新渲染替换掉 DOM 节点后
id 会变**，需要重新快照。

### 示例

```
ui_snapshot { "query": "play" }
→ { id: "u7", role: "button", label: "Play", bounds: {...} }

ui_click { "id": "u7" }
ui_highlight { "id": "u7", "ms": 2000 }
```

### 限制

- 只能看到**渲染后**的 DOM：虚拟列表窗口外的行不在快照里（日志面板就是窗口化的）。
- 快照默认上限 400 个节点，用 `max_nodes` / `query` 收窄。
- 没有等待/重试语义：点完立刻返回，状态变化要用 `ui_snapshot` 或 `ui_eval` 复查。
