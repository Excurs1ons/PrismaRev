use crate::util::timer::{TimerClient, TimerId, TimerParams, TimerSet};

/// UI 面板基类 —— 持有 TimerSet + 子面板树 + 可见性。
pub struct PanelBase {
    /// 定时器集合（自动缓存 id，Drop 时批量销毁）。
    pub timers: TimerSet,
    /// 子面板。
    pub children: Vec<Box<dyn super::Panel>>,
    /// 是否可见/激活。
    pub visible: bool,
}

impl PanelBase {
    pub fn new(client: TimerClient) -> Self {
        Self {
            timers: TimerSet::new(client),
            children: Vec::new(),
            visible: true,
        }
    }

    // ── Timer 转发 ─────────────────────────────────────────

    /// 创建定时器（自动缓存 id，面板析构时自动清理）。
    pub fn create_timer(&mut self, params: TimerParams) -> TimerId {
        self.timers.create_timer(params)
    }

    pub fn destroy_timer(&mut self, id: TimerId) {
        self.timers.destroy(id);
    }

    pub fn pause_timer(&self, id: TimerId) {
        self.timers.pause(id);
    }

    pub fn resume_timer(&self, id: TimerId) {
        self.timers.resume(id);
    }

    /// 清空本面板所有定时器（关闭面板时调用）。
    pub fn clear_timers(&mut self) {
        self.timers.clear();
    }
}
