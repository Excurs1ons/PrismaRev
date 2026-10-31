//! 把 `log` 记录同时转发到编辑器的 Console 面板。
//!
//! `env_logger` 仍然负责终端输出（`RUST_LOG` 语义不变）；这里再包一层
//! `log::Log`，把同一条记录塞进一个环形缓冲，`EditorApp` 每帧取走写进
//! `prism_editor_core::LogBuffer`（面板按来源 / 级别过滤 + 窗口化渲染）。
//!
//! 不加这个的话，视口里的 `prism_render` / `prism_engine` 警告只能在终端里看到，
//! 与 Web 编辑器（进程内引擎日志进 Console）不一致。

use std::collections::VecDeque;
use std::panic::PanicHookInfo;
use std::sync::{Mutex, OnceLock};

use prism_editor_core::{LogLevel, LogSource};

const CAPACITY: usize = 4_000;

struct Entry {
    source: LogSource,
    level: LogLevel,
    text: String,
}

/// 工程模块（preview module）的日志目标前缀。
///
/// `prism-runtime-host` 把模块通过 `HostApi::log` 发出的记录转成
/// `project: ...`；`panic` 来自本文件的崩溃 hook。两者都属于「游戏进程」，
/// Console 面板按来源过滤时要和引擎日志分开。
fn source_for(target: &str) -> LogSource {
    if target == "panic" || target.starts_with("project") {
        LogSource::Game
    } else {
        LogSource::Engine
    }
}

static SINK: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

fn sink() -> &'static Mutex<VecDeque<Entry>> {
    SINK.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

struct Tee {
    inner: env_logger::Logger,
}

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata) || metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }
        // 只把「编辑器自己关心」的级别送进面板，Trace 会淹没窗口。
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Trace => return,
        };
        if let Ok(mut queue) = sink().lock() {
            if queue.len() >= CAPACITY {
                queue.pop_front();
            }
            queue.push_back(Entry {
                source: source_for(record.target()),
                level,
                text: format!("{}: {}", record.target(), record.args()),
            });
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// 安装 tee logger 与崩溃 hook。重复调用是 no-op。
pub fn install(default_filter: &str) {
    if SINK.get().is_some() {
        return;
    }
    // 先占位，避免递归初始化。
    sink();
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
            .format_timestamp(None)
            .build();
    let tee = Tee { inner };
    let max = log::LevelFilter::Debug;
    if log::set_boxed_logger(Box::new(tee)).is_ok() {
        log::set_max_level(max);
    }
    install_panic_hook();
}

/// 把 panic 的调用栈送进 Console 面板。
///
/// 编辑器在 Play 时会把工程代码作为动态模块加载进**同一个进程**，所以工程里的
/// `unwrap()` 会走到这里。默认 hook 只往 stderr 打印，面板看不到；这里额外抓
/// 一份 `Backtrace`（`force_capture`，不依赖 `RUST_BACKTRACE`）。
///
/// 模块帧能不能解出符号取决于 PDB：`project_module::stage` 会把 `.pdb` 和
/// `.dll` 一起复制到暂存目录，dbghelp 按模块目录查找同名符号文件。
fn install_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
            push_panic(info);
            // 保留原有行为（stderr / RUST_BACKTRACE），终端排查仍然可用。
            previous(info);
        }));
    });
}

fn push_panic(info: &PanicHookInfo<'_>) {
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "<unknown location>".to_owned());
    // 在 hook 里抓栈：此时还没有开始 unwind，帧是完整的（包含工程模块的帧）。
    let backtrace = std::backtrace::Backtrace::force_capture();
    if let Ok(mut queue) = sink().lock() {
        if queue.len() >= CAPACITY {
            queue.pop_front();
        }
        queue.push_back(Entry {
            source: LogSource::Game,
            level: LogLevel::Error,
            text: format!("panic at {location}: {payload}\n{backtrace}"),
        });
    }
}

/// 取走缓冲里的新记录（每帧调用一次）。
pub fn drain() -> Vec<(LogSource, LogLevel, String)> {
    let Ok(mut queue) = sink().lock() else {
        return Vec::new();
    };
    queue
        .drain(..)
        .map(|entry| (entry.source, entry.level, entry.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_preserves_levels_and_source() {
        sink().lock().unwrap().clear();
        sink().lock().unwrap().push_back(Entry {
            source: LogSource::Engine,
            level: LogLevel::Warn,
            text: "engine warning".to_owned(),
        });
        let entries = drain();
        assert_eq!(
            entries,
            vec![(
                LogSource::Engine,
                LogLevel::Warn,
                "engine warning".to_owned()
            )]
        );
        assert!(drain().is_empty());
    }

    #[test]
    fn project_and_panic_targets_are_game_source() {
        assert_eq!(source_for("project"), LogSource::Game);
        assert_eq!(source_for("project:world"), LogSource::Game);
        assert_eq!(source_for("panic"), LogSource::Game);
        assert_eq!(source_for("prism_engine::scene"), LogSource::Engine);
        assert_eq!(source_for("prism_editor_egui::app"), LogSource::Engine);
    }
}
