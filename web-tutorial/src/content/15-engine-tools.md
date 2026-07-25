# 15 · 音频与调试子系统

引擎不止有渲染。一个可玩的游戏引擎还需要**音频**和**调试工具**。本章介绍 `prism-audio` 音频子系统和 egui 驱动的调试覆盖层。

---

## 音频子系统：prism-audio

引擎的音频子系统在 `crates/prism-audio/` 中独立实现，使用 `firewheel`（Firelight Audio）作为底层音频图引擎，`cpal` 作为跨平台音频后端。

### AudioEngine

`AudioEngine`（`engine.rs`，375 行）是音频系统的核心：

```rust
pub struct AudioEngine { /* ... */ }

impl AudioEngine {
    pub fn new(config: &AudioConfig) -> Result<Self, AudioError>;
    pub fn play(&mut self, data: &AudioData) -> PlaybackHandle;
    pub fn stop(&mut self, handle: &PlaybackHandle);
    pub fn pause(&mut self, handle: &PlaybackHandle);
    pub fn resume(&mut self, handle: &PlaybackHandle);
    pub fn set_volume(&mut self, handle: &PlaybackHandle, volume: f32);
    pub fn update(&mut self, dt: f32);
    pub fn stop_all(&mut self);
}
```

`AudioConfig` 配置设备名称、采样率（默认 44100 Hz）、声道数（默认立体声）和主音量。

```rust
pub struct AudioConfig {
    pub device_name: Option<String>,  // None = 系统默认设备
    pub sample_rate: u32,             // 默认 44100
    pub channels: u16,                // 默认 2（立体声）
    pub master_volume: f32,           // 0.0~1.0
}
```

### 音频数据与解码

`AudioData` 存储解码后的音频样本：

```rust
pub struct AudioData {
    pub samples: Vec<f32>,     // 交错 f32 样本（L,R,L,R,…）
    pub sample_rate: u32,      // 采样率（Hz）
    pub channels: u16,         // 声道数（1=单声道，2=立体声）
    pub duration: Duration,    // 总时长
}
```

解码器（`decoder.rs`）支持多种格式：

```rust
pub enum AudioFormat { Wav, Ogg, Mp3, Flac }

pub fn decode_file(path: &Path) -> Result<AudioData, AudioError>;
pub fn decode_bytes(format: AudioFormat, data: &[u8]) -> Result<AudioData, AudioError>;
pub fn decode_auto(data: &[u8]) -> Result<AudioData, AudioError>;
```

### ECS 集成：AudioSource

`crates/prism-engine/src/audio.rs` 定义了 ECS 组件 `AudioSource`，让每个实体可以发声：

```rust
pub struct AudioSource {
    pub data: Option<AudioData>,  // 音频片段
    pub volume: f32,              // 音量 0.0~1.0
    pub playing: bool,            // 是否正在播放
    pub repeat: bool,             // 是否循环（暂未完全接入）
}
```

`sync_audio_sources` 系统函数每帧遍历所有 `AudioSource` 实体，驱动 `AudioEngine` 的方法：

- `playing && handle is None` → 开始播放
- `playing && handle exists` → 更新音量
- `!playing && handle exists` → 停止

### 优雅降级

当设备不可用（如无音频输出设备）时，`AudioEngine::new` 返回 `AudioError::DeviceNotFound`。引擎选择静默运行，不会因此崩溃。这不只是 bug 防护——在无音频的 CI 环境、远程服务器或部分 Android 模拟器上，这是必要的设计。

---

## 实时场景编辑器：Inspector

引擎的 `Inspector`（`inspector.rs`，681 行）是一个 egui 驱动的实时编辑面板，通过 F1 切换。它提供：

**实体列表**：左侧列出所有带 `Transform` 组件的实体，点击选中后可在右侧编辑其位置/旋转（欧拉角界面）/缩放。

**光源编辑**：`DirectionalLight` 的 XYZ 欧拉角/颜色/照度（lux）；`PointLight` 的位置/范围/颜色/强度（candela）。

**相机控制**：曝光值（0~5 滑块）、FOV、近/远裁剪面。

**调试面板**：

| 参数 | 说明 |
|------|------|
| `debug_mode` | 最终/Albedo/Specular/Reflect/Ambient/Normal 视图切换 |
| `render_mode` | Raster（PBR）↔ PathTrace 切换 |
| `pt_max_bounces` | 路径追踪最大 bounce 次数 |
| `pt_ray_max_distance` | PT 光线最大长度 |
| `pt_max_iterations` | PT 最大采样帧数（0 = 无限累积） |
| `tonemap_mode` | Reinhard ↔ ACES Narkowicz |
| `exposure` | HDR 曝光乘数 |
| 帧时间 / FPS | 实时性能监控 |

### EguiOverlay：两阶段架构

`EguiOverlay`（`egui_overlay.rs`，408 行）将 egui 渲染集成到 Vulkan 管线。核心挑战是**借用冲突**：UI 需要 `&mut World` 和 `&mut Camera`（在 `App` 中），而 GPU 录制在 `GraphRenderer::render` 中（需要 `&mut self`）。解决方案是两阶段拆分：

```
第一阶段：run_ui（App::render_one_frame 中）
  ├── 持有 &mut World + &mut Camera + &mut Inspector
  ├── 运行 egui::Context::run → UI 逻辑执行
  └── 镶嵌到 CPU 端图元列表 + 缓存纹理变化

第二阶段：record（GraphRenderer::render 内部）
  ├── 无 World/Camera 访问
  ├── 消费第一阶段缓存的图元列表
  └── 录制 cmd_draw → 提交到帧命令缓冲
```

```rust
// App::render_one_frame（简化）
fn render_one_frame(&mut self) {
    // 1. 更新场景
    // 2. Inspector UI（持有 &mut World/Camera/Inspector）
    if self.inspector.show {
        self.inspector.run(&mut self.camera, &mut self.world,
                           &self.input, &mut self.graph_renderer);
    }
    // 3. 渲染（持有 &mut GraphRenderer）
    self.graph_renderer.render(&self.frame_input);
}
```

```rust
// EguiOverlay 内部
pub fn run_ui(&mut self, window: &Window, run: impl FnOnce(&egui::Context));
pub fn record(&mut self, device: &ash::Device, cmd: vk::CommandBuffer,
              target_view: vk::ImageView, extent: vk::Extent2D);
```

### RenderGraphViz：F2 可视化

`RenderGraphViz`（`render_graph_viz.rs`，548 行）通过 F2 切换，将当前渲染管线的 pass 图可视化为一个节点-边图。它显示每个 `RenderPassNode` 的 inputs/outputs、资源格式和执行顺序——对理解第 7 章的管线编排非常有帮助。

---

## 跨平台崩溃对话框

引擎在不使用 `std::process::abort()` 的情况下捕获致命错误（device lost、验证层致命错误），弹出**原生模态对话框**。每个平台使用自己的原生 API：

| 平台 | 对话框 | 剪贴板 |
|------|--------|--------|
| Windows | `MessageBoxW`（`MB_YESNO`） | `OpenClipboard` / `SetClipboardData` |
| macOS | `osascript`（`display dialog`） | `pbcopy` |
| Linux | `zenity --question`（回退：stderr） | `xclip` / `xsel` |
| Android | JNI `AlertDialog` | 文本写入 logcat |

```rust
pub enum CrashChoice { CopyAndExit, Exit }

// crash_dialog.rs 入口
pub fn show_crash_dialog(title: &str, message: &str) -> CrashChoice;
```

用户可选择「Copy & Exit」（复制错误文本到剪贴板后退出，方便贴 issue）或直接「Exit」。整个对话框阻塞主线程直到用户确认，自然暂停渲染循环。

---

## 动手练习

:::exercise
1. 读 `crates/prism-audio/src/engine.rs` 的 `AudioEngine::play` 和 `stop` 方法，理解 `PlaybackHandle` 的生成和销毁生命周期。
2. 用 `AudioSource` 组件在场景中 spawn 一个会发声的实体，观察 `sync_audio_sources` 如何驱动 `AudioEngine`。
3. 按 F1 打开 Inspector，编辑方向光的 `euler_xyz` 角度，观察场景中阴影的实时变化。
4. 按 F2 打开 RenderGraphViz，对照渲染管线章（第 9 章）的 pass 列表，验证可视化图的结构。
5. 读 `egui_overlay.rs` 的 `run_ui` / `record` 两阶段设计，画出时序图解释为什么必须这样拆分。
6. 在引擎中制造一个 device lost（如移除 `drop_target` 调用），验证崩溃对话框正确弹出。
:::
