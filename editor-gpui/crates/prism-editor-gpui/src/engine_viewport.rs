//! 真实 Vulkan Scene View（可选 feature `engine-viewport`）。
//!
//! 在后台线程创建 `prism_app::App::hosted`，加载编辑器当前文档的临时
//! `.scene.json` 快照，按需 `host_tick`，把 RGBA 帧通过 channel 送回 UI。
//! 没有 Vulkan 设备时线程只发一次 `Error`，UI 回退到 CPU 投影。

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineFrame {
    pub width: u32,
    pub height: u32,
    /// RGBA8，行优先，左上角原点。
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum EngineUpdate {
    /// 引擎初始化完成（帧即将开始到达）。
    Ready,
    Frame(EngineFrame),
    Stats {
        fps: f32,
    },
    Error(String),
}

enum EngineCommand {
    Reload(PathBuf),
    Resize(u32, u32),
    Simulate(bool),
    Step,
    Shutdown,
}

pub struct EngineViewport {
    commands: Sender<EngineCommand>,
    updates: Receiver<EngineUpdate>,
    join: Option<JoinHandle<()>>,
}

impl EngineViewport {
    /// 启动后台引擎；`scene` 必须是已写盘的 `.scene.json` 快照。
    pub fn start(scene: PathBuf, width: u32, height: u32) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("gpui-engine-viewport".to_string())
            .spawn(move || engine_loop(scene, width.max(1), height.max(1), command_rx, update_tx))
            .ok();
        Self {
            commands: command_tx,
            updates: update_rx,
            join,
        }
    }

    pub fn reload(&self, scene: PathBuf) {
        let _ = self.commands.send(EngineCommand::Reload(scene));
    }

    pub fn resize(&self, width: u32, height: u32) {
        let _ = self
            .commands
            .send(EngineCommand::Resize(width.max(1), height.max(1)));
    }

    pub fn set_simulate(&self, simulate: bool) {
        let _ = self.commands.send(EngineCommand::Simulate(simulate));
    }

    pub fn step(&self) {
        let _ = self.commands.send(EngineCommand::Step);
    }

    /// 非阻塞收集本 tick 的更新。
    pub fn poll(&self) -> Vec<EngineUpdate> {
        self.updates.try_iter().collect()
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(EngineCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for EngineViewport {
    fn drop(&mut self) {
        let _ = self.commands.send(EngineCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn engine_loop(
    scene: PathBuf,
    width: u32,
    height: u32,
    commands: Receiver<EngineCommand>,
    updates: Sender<EngineUpdate>,
) {
    let config = prism_engine::config::AppConfig::default();
    let mut app = prism_app::App::hosted(config);
    if let Err(error) = app.load_scene_json_file(&scene) {
        let _ = updates.send(EngineUpdate::Error(format!("加载场景失败：{error}")));
        return;
    }
    app.start_hosted(width, height);
    let _ = updates.send(EngineUpdate::Ready);

    let mut simulate = false;
    let mut step_once = false;
    let mut dirty = true;
    let mut warmup = 3u8;
    let mut last_frame_at = std::time::Instant::now();
    let mut frame_count = 0u32;
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                EngineCommand::Reload(path) => {
                    if let Err(error) = app.load_scene_json_file(&path) {
                        let _ =
                            updates.send(EngineUpdate::Error(format!("重新加载场景失败：{error}")));
                    }
                    dirty = true;
                    warmup = 2;
                }
                EngineCommand::Resize(width, height) => {
                    app.host_resize(width, height);
                    dirty = true;
                    warmup = 2;
                }
                EngineCommand::Simulate(value) => {
                    simulate = value;
                    dirty = true;
                }
                EngineCommand::Step => {
                    step_once = true;
                    dirty = true;
                }
                EngineCommand::Shutdown => {
                    app.shutdown_hosted();
                    return;
                }
            }
        }

        // 只有模拟、场景变更或启动预热时才真正渲染 / 读回；静止的编辑器视口
        // 不再每帧走 GPU→CPU 回读。
        if simulate || step_once || dirty || warmup > 0 {
            app.host_tick(simulate || step_once);
            step_once = false;
            dirty = false;
            warmup = warmup.saturating_sub(1);

            if let Some(frame) = app.take_hosted_frame() {
                let _ = updates.send(EngineUpdate::Frame(EngineFrame {
                    width: frame.width,
                    height: frame.height,
                    pixels: frame.pixels,
                }));
                frame_count += 1;
            }
            if frame_count >= 15 {
                frame_count = 0;
                if let Some(stats) = app.render_stats() {
                    let _ = updates.send(EngineUpdate::Stats { fps: stats.fps });
                }
            }
        }
        if let Some(error) = app.render_failure() {
            let _ = updates.send(EngineUpdate::Error(error));
            app.shutdown_hosted();
            return;
        }

        // 30 Hz 足够编辑器视口；渲染线程自己按需出帧。
        let elapsed = last_frame_at.elapsed();
        last_frame_at = std::time::Instant::now();
        if elapsed < Duration::from_millis(33) {
            thread::sleep(Duration::from_millis(33) - elapsed);
        }
    }
}
