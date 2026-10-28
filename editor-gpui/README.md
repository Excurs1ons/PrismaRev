# editor-gpui — PrismaRev 的 GPUI 原生编辑器

> 一个**从零实现**的 Unity 形状编辑器外壳：dock、层级、检查器、资产、日志、场景视口
> 全部由 GPUI 直接绘制，不经过 WebView。
>
> 它有自己的 workspace（和 `editor/`、`launcher/` 一样不挂根 workspace），
> **不进入** 游戏运行时依赖链。

## 为什么是 GPUI

- **纯 Rust、GPU 加速**：没有 Rust ↔ TypeScript 状态双写，也没有 WebView 合成层。
- **`Render for View` + `cx.notify()`**：面板状态改完就重绘，和 ECS/反射数据天然亲近。
- **原生窗口**：将来把 Scene View 换成 `prism-render` 的真实 Vulkan surface 时，
  不需要再和 WebView2 的合成顺序搏斗。

与 Web 编辑器（`editor/`）的关系是「**同一份数据、两个外壳**」：

| | `editor/`（Tauri + Vue） | `editor-gpui/`（本目录） |
|---|---|---|
| 文档模型 | `prism-editor-api` | `prism-editor-gpui-core::doc`（同形状） |
| dock 布局 | `dock.ts`（v4 JSON） | `dock.rs`（同一份 v4 JSON） |
| 主题 | `theme.ts`（38 token × 6 预设） | `theme.rs`（机械移植，同 id） |
| 文案 | `locales/*.ts` | `locales/*.json`（机械转换，331 条） |
| 快捷键 | `App.vue` 菜单表 | `keys.rs`（`Command` + `SHORTCUTS`） |
| fixture | `pnpm fixture` → `editor/public/demo.json` | `scripts/sync-demo.sh` 同步同一份 |

## 已实现

**外壳**

- 菜单栏（文件/编辑/构建/窗口/帮助）与快捷键（`Ctrl+S/Z/Shift+Z/D/A/V`、`Del`、`F`、`Home`…）
- 工具栏：布局预设（Default / 2 by 3 / Tall / Wide）、文档标签、Play/Pause/Step/Stop、
  取景（全部 / 选中）、主题与语言切换、状态栏（脏标记 / 实体数 / 选中数 / 错误与告警计数）
- **split-tree docking**：同侧 ratio 共享、splitter 拖拽、tab 拖到 leaf 中央合并/split、
  拖到主窗口三边新开整列/整行、拖出窗口变浮动面板、8 方向 resize、磁吸、z-order
- 浮动窗口标题栏拖动、关闭；`window/toggle/<id>` 与「显示面板」菜单
- 右键菜单（层级实体 / 组件卡 / 资产）：复制、删除、重命名、**存为预制体**、
  还原 / 同步 / 解除预制体实例、复制/移除/粘贴组件、打开文档、重命名资产、复制路径
- **真实文件 IO**：自动从 cwd / 可执行文件目录向上找 `assets/`；打开磁盘上的
  `assets/scenes/*.scene.json`、`assets/prefabs/*.prefab.json`；`Ctrl+S` 保存
  （未命名文档自动落到默认路径）、`Ctrl+Shift+S` 另存为（弹层输入相对路径）、
  重新读取当前文档
- **自动保存**：首选项开启时每 5s 保存有改动的当前文档（Play 中跳过）
- **资产重命名 / 移动**：只允许 `assets/` 内；改磁盘文件并把打开文档里精确匹配的路径
  字段一起改写（不做子串替换），撤销仍然可用（文档标记 dirty）
- **存为预制体**：把选中实体子树写成 `assets/prefabs/<名字>.prefab.json`（重名加 `-2`），
  场景不变；资产右键可把 `.prefab.json` **实例化到场景**（挂到当前选中实体下）
- **本地状态持久化**：dock 布局 / 主题（预设 + override）/ 语言 / 首选项写入
  `<project>/target/editor-gpui/state.json`，下次启动恢复；损坏或未知字段静默回退

**面板**

| 面板 | 内容 |
|---|---|
| 层级 | 实体树、搜索（匹配项忽略折叠、按层级缩进）、展开/折叠、多选（Shift 区间 / Ctrl 切换）、双击或 F2 重命名、眼睛切 Active、拖拽改父级/排序（上 25% / 下 25% / 中间）、按预设新建 |
| 检查器 | 实体面包屑 + schema 驱动：bool / int / float / 滑杆 / 向量 / 四元数 / 颜色 / 枚举 / 文本 / 只读；拖动调值（Shift+点击输入）、组件卡折叠、复制/移除、添加组件（分类 + 搜索）、无 schema 时显示原始 JSON；**多选时只显示公共组件，字段修改一次应用到全部（一次撤销回退）** |
| 资产 | 目录树 + 文件列表、分类 chips、搜索、选中详情（导入器 / 体积 / 分类）；右键**重命名 / 移动**（磁盘改名 + 打开文档里的精确路径引用自动改写） |
| 日志 | 级别 × 来源过滤、搜索、错误/告警计数、窗口化 `uniform_list`（固定 18px 行高） |
| 场景视图 | `canvas` 绘制的世界网格、实体线框、选中高亮 + gizmo 三轴；**点击拾取实体**、**双击聚焦**、**拖 gizmo 移动**（Ctrl 吸附 0.25）、空白处左键轨道、右键平移、滚轮缩放 |
| 游戏视图 | 文档相机视角 + Play 状态；Play 时实体轻微上下浮动（模拟 tick） |
| 文档 | 打开的文档、新建场景/预制体、模板 |
| 渲染图 | pass 链（ShadowMap → Forward → Gtao → Post）+ 图资源表（scene/swapchain/frame 生命周期） |
| 项目设置 / 首选项 / 主题 / 构建 / 离线工具 / 快捷键 / 关于 | 直接编辑设置、主题预设与 38 个 token 预览、构建步骤日志、bake-gi 与 heightmap 命令预览 |

**数据层**（`prism-editor-gpui-core`，UI 框架无关，111 个单元测试）

- `.scene.json` / `.prefab.json`：稳定 id、parent 迁移、删除孤儿子级变根、reparent 防环、
  子树提取（存预制体）、预制体实例化（id 平移 + 名字去重 + 反向链接）、字段引用扫描
- 撤销栈：字段级合并（同一实体/组件/路径 800ms 内合并）、Transform 单独合并、结构编辑用整文档快照
- 多对象编辑：公共组件求交、批量字段写入（一次快照撤销）、批量移除组件
- 视口数学：世界↔屏幕投影、屏幕射线、AABB 拾取、gizmo 轴命中与像素↔世界换算
- 本地状态：`EditorState`（布局 + 主题 + 语言 + 首选项）的宽容读写与回退
- 项目设置：`assets/settings.toml` 的 `[app]` / `[window]` / `[build]` 读写（合并写回，保留其它段）
- 资产重命名 / 移动：文件改名 + 文档引用改写（精确匹配，含嵌套数组/对象）
- 存为预制体：子树提取 + 名字清洗 + 重名去重 + 落盘
- 预制体实例化到场景：磁盘优先 / fixture 回退模板，自动挂到选中实体并选中新根
- 项目文件层：向上找 `assets/`、扫描 / 读写文档、默认保存路径防重名、
  预制体实例还原（保留名字与父级）与同源同步（磁盘优先，fixture 模式回退模板）
- 主题（38 token + 6 预设）、i18n（331 条 × 2 语言，key 集合有测试兜底）、
  日志环形缓冲（窗口化区间）、资产目录树、dock v4 读写与 `sanitize`

## 构建 / 运行

GPUI 是桌面框架（Windows / macOS / Linux），**不能在 Termux/Android 上运行**；
本机（aarch64 Android）也没有 X11/Wayland。仓库里的验证方式是交叉**类型检查**：

```sh
cd editor-gpui

# 数据层：本机直接跑
cargo test -p prism-editor-gpui-core

# UI 层：GPUI 全量类型检查（Windows 目标，不需要链接器 / 显示）
cargo check -p prism-editor-gpui --target x86_64-pc-windows-gnu
cargo clippy -p prism-editor-gpui -p prism-editor-gpui-core --target x86_64-pc-windows-gnu
```

在桌面主机上直接运行：

```sh
cd editor-gpui
cargo run -p prism-editor-gpui                 # 自动找 editor/public/demo.json，否则用内嵌 fixture
cargo run -p prism-editor-gpui -- --fixture ../../editor/public/demo.json
```

启动时会从 **cwd**（其次可执行文件目录）向上找 `assets/`：找到就进入「项目模式」
（文档列表来自磁盘、`Ctrl+S` 真正写文件）；找不到则是只读 fixture 模式
（状态栏显示「只读 fixture」，保存会报错）。

fixture 同步（Web 侧 schema 变了以后）：

```sh
cd editor && pnpm fixture
editor-gpui/scripts/sync-demo.sh
```

内嵌 fixture 是 `editor-gpui/assets/demo.json`（`include_str!`），所以只拷二进制也能启动。

## 目录

```
editor-gpui/
├── Cargo.toml                     # 独立 workspace
├── assets/demo.json               # 内嵌 fixture（与 editor/public/demo.json 同源）
├── scripts/sync-demo.sh
└── crates
    ├── prism-editor-gpui-core     # 无 UI 依赖：dock/doc/schema/undo/theme/i18n/log/assets/keys/workspace
    └── prism-editor-gpui          # GPUI 视图：app.rs / dock_view.rs / style.rs / panels/*
```

`*_panel` 都是 `impl EditorView` 上的方法（按面板分文件），共享 `Workspace` 与
`Style`（主题 token → `Hsla`）。

## 已知限制（刻意的下一步）

- **Scene View 目前是 CPU 投影的 `canvas` 绘制**，不是 `prism-render` 的真实画面。
  接线点已经留好：把 `paint_scene` 换成离屏 `GraphRenderer` 的帧（或原生 Vulkan
  popup 窗口 + 输入转发），面板本身不需要改。
- **删除仍是占位**：删除文档 / 资产、真实构建与 cook 只写日志（构建面板步骤是占位）。
  保存 / 另存为 / 重新读取 / settings.toml / 资产重命名与移动已真实落盘。
- **文本输入是最小实现**：单击/双击进入内联编辑，支持退格/回车/Esc 与普通字符，
  没有 IME、选区、光标移动；搜索框同理。补 IME 时需要 `ElementInputHandler`。
- 图标用文本符号（`▶`/`◉`/`▾`…），没有引入图标库；Web 编辑器用 `lucide-vue-next`。
- 渲染图面板是**静态快照**（形状与 `prism-render-graph.json` 一致），
  尚未读取真实报告。
- 主题面板可切预设并预览 token，但逐项覆盖（override）的编辑器还没做 UI。

## 验证记录

- `cargo test -p prism-editor-gpui-core`：**111 passed**。
- `cargo check -p prism-editor-gpui --target x86_64-pc-windows-gnu`：通过。
- `cargo clippy -p prism-editor-gpui -p prism-editor-gpui-core --target x86_64-pc-windows-gnu`：
  无 warning（仅 `proc-macro-error2` 的 future-incompat 提示）。
