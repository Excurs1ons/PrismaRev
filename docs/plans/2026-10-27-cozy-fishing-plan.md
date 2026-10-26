# Cozy Fishing 实施计划（M0）

> 设计文档：`docs/plans/2026-10-27-cozy-fishing-design.md`
> 规则：一个任务 = 一个分支 = 一个 PR；默认 diff < ~500 行；合并前跑 `python scripts/pr_ready.py`。

---

## 阶段划分

| 阶段 | 分支 | 内容 | 依赖 |
|---|---|---|---|
| P1 | `feat/sprite-renderer-component` | 精灵组件与 schema | — |
| P2 | `feat/sprite-pass` | 精灵渲染通道 + 着色器 + tonemap None | P1 |
| P3 | `feat/fishing-project-scaffold` | `projects/fishing` 工程 + 湖面场景 + 相机 | P2 |
| P4 | `feat/line-renderer` | 线渲染组件（鱼线） | P2 |
| P5 | `feat/audio-loops` | 循环播放 + 命名通道 | — |
| P6 | `feat/time-of-day` | 昼夜系统 | — |
| P7 | `feat/save-game` | 键值存档 | — |
| P8 | `feat/fishing-gameplay` | 钓鱼状态机 + HUD（用户项目） | P3–P7 |

P4–P7 与 P3 可并行，但按上表顺序合并以减少冲突。

---

## P1 — 精灵组件与 schema

**分支**：`feat/sprite-renderer-component`

**文件**

- 修改 `crates/prism-scene/src/components.rs`：新增 `SpriteRenderer`（`#[reflect(name = "prism_engine::scene::SpriteRenderer")]`，`#[serde(default)]`）
- 修改 `crates/prism-scene/src/component_registry.rs`：`register_builtin_components` 注册
- 修改 `crates/prism-engine/src/scene/runtime_components.rs`：新增 `SpriteRef`
- 测试：`crates/prism-scene/src/components.rs` 或现有 tests 模块加 JSON 往返

**验收**

- [ ] `.scene.json` 写 `prism_engine::scene::SpriteRenderer` 能 cook + load 不报错
- [ ] 缺省字段回落到默认值（跨版本兼容）
- [ ] `cargo test -p prism-scene` 通过
- [ ] 编辑器检查器自动出现该组件（注册表驱动，无需前端改动）

**注意**：不要动 `DERIVED_COMPONENTS`（`SpriteRef` 是运行时组件，不进作者格式）。

---

## P2 — 精灵渲染通道

**分支**：`feat/sprite-pass`

**文件**

- 新增 `assets/shaders/slang/sprites.slang`（vertex + fragment，set 1 bindless）
- 修改 `assets/shaders/compile.sh`：加 `sprites.slang` 编译 + `emit_reflection`
- 生成 `assets/shaders/reflection/sprites.json`，跑 `shader-bindgen` 生成 `crates/prism-render/src/shader_bindings/sprites.rs`
- 新增 `crates/prism-render/src/sprite_pass.rs`（`SpritePass`，实现 `RenderPassNode`）
- 修改 `crates/prism-render/src/render_graph.rs`：`SpriteDrawItem` 结构 + `GraphFrame.sprite_items`
- 修改 `crates/prism-render/src/graph_renderer.rs`：`FrameInput.sprite_items`，注册 pass
- 修改 `crates/prism-render/src/asset_bridge.rs`：请求/结果加纹理字段
- 修改 `crates/prism-engine/src/asset_resolver.rs`：为 `SpriteRenderer` 投递纹理请求
- 修改 `crates/prism-engine/src/scene/scene_system.rs`：`SceneReadView::sprite_items()`（排序）
- 修改 `crates/prism-engine/src/render_system.rs`：填充 `sprite_items`
- 修改 `assets/shaders/slang/post.slang` + `post.rs`：`tonemap_mode == 2` 直通

**验收**

- [ ] 场景里一张 32×32 PNG 在正交相机下按 1:1 像素显示（nearest，无模糊）
- [ ] 两个精灵按 `sorting_layer` / `order_in_layer` 正确前后遮挡
- [ ] `tonemap_mode = 2` 时颜色与源图一致（截图对比）
- [ ] 现有 3D 场景（`default` / `sponza`）渲染无回归
- [ ] `cargo check -p prism-render && cargo test -p prism-render`

---

## P3 — 用户工程脚手架

**分支**：`feat/fishing-project-scaffold`

**文件**

- 新增 `projects/fishing/Cargo.toml`（独立 workspace，依赖 `prism-app` / `prism-engine`）
- 新增 `src/main.rs`、`src/lib.rs`（含 Android `android_main`）
- 新增 `assets/scenes/lake.scene.json`（相机 + 天空/水/船精灵占位）
- 修改根 `assets/scenes.toml`？**否** —— 用户项目自带 `assets/`，通过项目目录运行

**验收**

- [ ] `cd projects/fishing && cargo run` 出现湖面场景
- [ ] 相机为 ortho，窗口缩放不拉伸
- [ ] 船可用 WASD 移动，相机平滑跟随

---

## P4 — 线渲染

**分支**：`feat/line-renderer`

- `prism-scene`：`LineRenderer` 组件 + 注册
- `prism-render`：折线 → 四边形条带展开（复用 `SpritePass` 实例缓冲）
- `prism-engine`：`SceneReadView::line_items()`

**验收**：3 点折线能画出连续、无断裂的线；宽度随世界单位正确缩放。

---

## P5 — 音频循环与通道

**分支**：`feat/audio-loops`

- `prism-audio`：`AudioClip { looped }`、`play_looping`、命名通道与音量
- `prism-engine/src/audio.rs`：把通道/循环暴露给用户项目

**验收**：环境音循环无缝（无爆音）；`music` 通道静音不影响 `sfx`。

---

## P6 — 昼夜

**分支**：`feat/time-of-day`

- `prism-scene`：`TimeOfDay` 组件 + 注册
- `prism-engine`：`time_of_day_system` 推进时间，驱动方向光与全局色调
- `prism-render`：`PostPass` 颜色分级（tint / 亮度）推送常量

**验收**：`day_length_seconds = 120` 时，2 分钟内天色从白天过渡到夜晚再回来，无跳变。

---

## P7 — 存档

**分支**：`feat/save-game`

- `prism-engine/src/save.rs`：`SaveGame`（版本化 JSON，用户数据目录）
- 平台路径：桌面用 `dirs`/`std::env`，Android 用 `internal_data_path`

**验收**：写入→重启→读取一致；版本号不匹配时走迁移钩子而不是 panic。

---

## P8 — 钓鱼玩法（用户项目）

**分支**：`feat/fishing-gameplay`

- `fishing.rs` 状态机（Idle/Casting/Waiting/Biting/Reeling）
- `fish_db.rs`（`assets/fish.json` 驱动）
- `hud.rs`（提示、图鉴计数）
- `boat.rs`（漂移 + 相机跟随）

**验收（M0 出口）**

- [ ] 3 分钟内能完成至少 3 次完整「抛竿 → 上鱼」
- [ ] 鱼种按权重随机，图鉴计数正确
- [ ] 退出重进后图鉴保留
- [ ] 60fps（1080p，集显）

---

## 自检清单（每个 PR）

1. `python scripts/pr_ready.py --quick`
2. 涉及渲染时额外跑 `cargo check -p prism-render` 与一次实机截图
3. 涉及场景格式时跑 `cargo test -p prism-scene -p prism-cooker`
4. PR 首行标注 AI 来源；不冒充人类
