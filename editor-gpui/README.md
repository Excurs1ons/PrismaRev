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

- 菜单栏（文件/编辑/构建/窗口/帮助）与快捷键（`Ctrl+S/Z/Shift+Z/C/X/V/D/A`、`Del`、`F`、`Home`…）
- 工具栏：布局预设（Default / 2 by 3 / Tall / Wide）、文档标签、Play/Pause/Step/Stop、
  取景（全部 / 选中）、主题与语言切换、状态栏（脏标记 / 实体数 / 选中数 / 错误与告警计数）
- **split-tree docking**：同侧 ratio 共享、splitter 拖拽、tab 拖到 leaf 中央合并/split、
  拖到主窗口三边新开整列/整行、拖出窗口变浮动面板、8 方向 resize、磁吸、z-order
- 浮动窗口标题栏拖动、关闭；`window/toggle/<id>` 与「显示面板」菜单
- 右键菜单（层级实体 / 组件卡 / 资产）：复制 / 剪切 / 粘贴 / 粘贴为子级 / 创建副本、
  删除、重命名、**存为预制体**、
  应用 / 还原 / 同步 / 解除预制体实例、复制/移除/粘贴组件、打开文档、重命名资产、复制路径
- **真实文件 IO**：自动从 cwd / 可执行文件目录向上找 `assets/`；打开磁盘上的
  `assets/scenes/*.scene.json`、`assets/prefabs/*.prefab.json`；`Ctrl+S` 保存
  （未命名文档自动落到默认路径）、`Ctrl+Shift+S` 另存为（弹层输入相对路径）、
  重新读取当前文档；文档标签右侧 ✕ 关闭，有未保存修改时先问「保存并关闭 / 不保存 / 取消」
- **自动保存**：首选项开启时每 5s 保存有改动的当前文档（Play 中跳过）
- **项目资产扫描**：项目模式启动时递归扫描整个 `assets/`（最多 12 层），填充资产面板
  与检查器资产选择器；扩展名决定分类 / 导入器
- **检查器资产选择器**：`*_path` / `env_map` / `volume` / `prefab` 字段右侧「…」按字段名
  过滤类别（模型 / 纹理 / 环境 / 音频 / 材质 / 预制体 / 场景），单击写回路径，可清除
- **资产重命名 / 移动**：只允许 `assets/` 内；改磁盘文件并把打开文档里精确匹配的路径
  字段一起改写（不做子串替换），撤销仍然可用（文档标记 dirty）
- **存为预制体**：把选中实体子树写成 `assets/prefabs/<名字>.prefab.json`（重名加 `-2`），
  场景不变；资产右键可把 `.prefab.json` **实例化到场景**（挂到当前选中实体下）
- **应用预制体修改**：实例检查器的「应用」/ 右键「应用预制体修改」把实例子树覆盖写回源
  预制体（剥掉根上的实例链接，嵌套实例链接保留），不影响场景
- **本地状态持久化**：dock 布局 / 主题（预设 + override）/ 语言 / 首选项写入
  `<project>/target/editor-gpui/state.json`，下次启动恢复；损坏或未知字段静默回退

**面板**

| 面板 | 内容 |
|---|---|
| 层级 | 实体树、搜索（匹配项忽略折叠、按层级缩进）、展开/折叠（`Alt`+点箭头递归展开整棵子树）、多选（Shift 按可见行区间 / Ctrl 切换）、双击或 F2 重命名（一次撤销 / Esc 回滚）、眼睛切 Active、拖拽改父级/排序（上 25% / 下 25% / 中间，多选拖中间整组）、按预设新建、Ctrl+D 复制整棵子树 |
| 检查器 | 实体面包屑 + schema 驱动：bool / int / float / 滑杆 / 向量 / 四元数 / 颜色 / 枚举 / 文本 / 只读；拖动调值（Shift+点击输入）、组件卡折叠、复制/移除、添加组件（分类 + 搜索）、无 schema 时显示原始 JSON；**多选时显示 Transform + 公共组件，字段修改一次应用到全部（一次撤销回退）** |
| 资产 | 目录树 + 文件列表、分类 chips、搜索、选中详情（导入器 / 体积 / 分类）；**双击场景/预制体打开文档**；**拖资产到检查器字段写路径、拖预制体到层级实体实例化**；右键**重命名 / 移动**（磁盘改名 + 打开文档里的精确路径引用自动改写）与**删除**（二次确认，不清理悬空引用） |
| 日志 | 级别 × 来源过滤、搜索、错误/告警计数、窗口化 `uniform_list`（固定 18px 行高） |
| 撤销历史 | 当前文档的撤销栈（最近 → 最早），点击某条回退到该操作之前；一键清空 |
| 场景视图 | `canvas` 绘制的世界网格（`G` 开关）、实体线框、选中高亮；**点击拾取实体**、**双击聚焦**、空白处左键拖**框选**（Shift 追加）、`Alt`+左键或右键轨道旋转、中键平移、滚轮缩放；gizmo 三模式（`W` 移动 / `E` 旋转 / `R` 缩放，Ctrl 吸附 0.25）：移动与缩放拖轴、旋转拖圆环，均可撤销（`X` 切世界/局部） |
| 游戏视图 | 文档相机视角 + Play 状态；Play 时实体轻微上下浮动（模拟 tick） |
| 文档 | 打开的文档、新建场景/预制体、模板 |
| 渲染图 | pass 链（ShadowMap → Forward → Gtao → Post）+ 图资源表（scene/swapchain/frame 生命周期） |
| 项目设置 / 首选项 / 主题 / 构建 / 离线工具 / 快捷键 / 关于 | 直接编辑设置、主题预设与 38 个 token 预览、真实 cargo/cook 构建日志、bake-gi 与 heightmap 命令预览 |

**数据层**（`prism-editor-gpui-core`，UI 框架无关，179 个单元测试）

- `.scene.json` / `.prefab.json`：稳定 id、parent 迁移、删除孤儿子级变根、reparent 防环、
  子树提取（存预制体）、预制体实例化（id 平移 + 名字去重 + 反向链接）、字段引用扫描
- 撤销栈：字段级合并（同一实体/组件/路径 800ms 内合并）、Transform 单独合并、结构编辑用整文档快照
- 多对象编辑：公共组件求交、批量字段写入（一次快照撤销）、批量移除组件、
  多选 Transform（同一局部分量写到全部）
- 多选 gizmo：平移把同一世界位移应用到所有选中实体（各自换算父空间）；旋转绕各自
  位置、缩放保留各自起始比例；三种模式整次拖动都合并成一个撤销步
- 子树复制：内部 parent 重映射、根保持父级、名字加 `(1)`；重命名预览不产生撤销步
- 实体剪贴板：Ctrl+C/X/V、右键「粘贴 / 粘贴为子级」、多根批量复制；预制体实例
  复制后保留同源链接（`extract_subtree_preserving_links`）
- 多选拖拽改父级：拖到行中间时 `reparent_selection` 把选中集合的顶层实体一起挂过去
  （一次快照撤销，挂到自己后代下会被拒绝）
- 视口数学：世界↔屏幕投影、屏幕射线、AABB 拾取、gizmo 轴/圆环命中、像素↔世界换算、
  四元数世界/局部轴旋转、父链世界旋转与父空间位移换算
- 本地状态：`EditorState`（布局 + 主题 + 语言 + 首选项）的宽容读写与回退
- 项目设置：`assets/settings.toml` 的 `[app]` / `[window]` / `[build]` 读写（合并写回，保留其它段）
- 场景清单：`assets/scenes.toml` 顺序/enabled/启动场景、文档路径映射、名字去重、构建面板编辑
- 资产重命名 / 移动：文件改名 + 文档引用改写（精确匹配，含嵌套数组/对象）
- 资产删除：只允许 `assets/` 内的普通文件（场景/预制体走文档流程）
- 存为预制体：子树提取 + 名字清洗 + 重名去重 + 落盘
- 预制体实例化到场景：磁盘优先 / fixture 回退模板，自动挂到选中实体并选中新根
- 打开预制体：实例根检查器的 P 徽章与实体右键「打开预制体」直接切到源预制体文档
- 项目文件层：向上找 `assets/`、递归扫描文档与资产（分类/导入器）、默认保存路径防重名、
  预制体实例还原（保留名字与父级）与同源同步（磁盘优先，fixture 模式回退模板）
- 资产字段判定：`assets::is_asset_path_field`（`*_path` + `env_map`/`volume`/`prefab`）
  与 `Workspace::asset_picker_candidates`（按字段名推断类别）
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
- **构建是真实子进程**：构建面板 / 「构建项目」「构建场景包」菜单按 `core::build` 的计划执行
  `cargo build --manifest-path <game>/Cargo.toml [--release]` 与
  `cargo run -p prism-build-pipeline -- build-scenes`，stdout/stderr 流式进构建日志与日志面板；
  成功后把 exe / `scenes.pak` / `settings.toml` 拷到 `[build].output_dir`（缺失的产物跳过）。
  子进程在后台线程跑，UI 每 tick 消费最多 40 行；构建面板头部显示当前步骤并画出分段进度条。桌面运行时回归尚未做。
  文档删除已真实落盘（菜单「文件 → 删除文档」或资产面板右键，都会先确认，
  并同时清理打开标签与 `scenes.toml` 条目）。
  保存 / 另存为 / 重新读取 / settings.toml / scenes.toml / 资产改名、删除、实例化与预制体写入已真实落盘。
- **文本输入**走 `prism-editor-gpui-core::TextInput`（光标 / 选区 / 按词移动与删除，
  Unicode 安全），但**没有 IME**，也还不画插入符与选区高亮（需要文本测量）。
  补 IME 时需要 `ElementInputHandler`。
- 图标用文本符号（`▶`/`◉`/`▾`…），没有引入图标库；Web 编辑器用 `lucide-vue-next`。
- gizmo 支持**世界 / 局部**两种坐标（`X` 或视口标题栏按钮切换，选中实体按父链累积旋转）；
  旋转在局部空间右乘、平移经父级旋转逆换算，父级旋转后也不会漂。
- 渲染图面板是**静态快照**（形状与 `prism-render-graph.json` 一致），
  尚未读取真实报告。
- 主题面板可切预设并逐 token 编辑颜色覆盖（只处理 RGBA token），但不能导入/导出主题文件。

## 验证记录

- `cargo test -p prism-editor-gpui-core`：**179 passed**。
- `cargo check -p prism-editor-gpui --target x86_64-pc-windows-gnu`：通过。
- `cargo clippy -p prism-editor-gpui -p prism-editor-gpui-core --target x86_64-pc-windows-gnu`：
  无 warning（仅 `proc-macro-error2` 的 future-incompat 提示）。
- `python scripts/pr_ready.py`：改动 `editor-gpui/` 时自动跑数据层单测；装了
  `x86_64-pc-windows-gnu` 目标时再跑 UI 类型检查。CI（`.github/workflows/ci.yml`）
  新增 `editor-gpui` job 跑数据层单测。
