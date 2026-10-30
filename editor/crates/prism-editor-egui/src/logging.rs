//! 把 `log` 记录同时转发到编辑器的 Console 面板。
//!
//! `env_logger` 仍然负责终端输出（`RUST_LOG` 语义不变）；这里再包一层
//! `log::Log`，把同一条记录塞进一个环形缓冲，`EditorApp` 每帧取走写进
//! `prism_editor_core::LogBuffer`（面板按来源 / 级别过滤 + 窗口化渲染）。
//!
//! 不加这个的话，视口里的 `prism_render` / `prism_engine` 警告只能在终端里看到，
//! 与 Web 编辑器（进程内引擎日志进 Console）不一致。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use prism_editor_core::{LogLevel, LogSource};

const CAPACITY: usize = 4_000;

struct Entry {
    level: LogLevel,
    text: String,
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
                level,
                text: format!("{}: {}", record.target(), record.args()),
            });
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// 安装 tee logger。重复调用是 no-op。
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
}

/// 取走缓冲里的新记录（每帧调用一次）。
pub fn drain() -> Vec<(LogSource, LogLevel, String)> {
    let Ok(mut queue) = sink().lock() else {
        return Vec::new();
    };
    queue
        .drain(..)
        .map(|entry| (LogSource::Engine, entry.level, entry.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_preserves_levels_and_maps_source_to_engine() {
        sink().lock().unwrap().clear();
        sink().lock().unwrap().push_back(Entry {
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
}
