//! 主线程定时器服务 —— 创建 / 暂停 / 恢复 / 销毁
//!
//! 存储使用 SlotMap（Vec<Option<T>> + free list），u32 slot 索引为 ID，无哈希。
//!
//! # 用法
//! ```ignore
//! let mut timer = TimerService::new();
//! let h = timer.client.create_timer(TimerParams { .. });
//! timer.pause(h); timer.resume(h); timer.destroy(h);
//! timer.tick(); // 主循环每帧
//! ```

use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

// ── SlotMap ──────────────────────────────────────────────────────────────

struct SlotMap<T> {
    pub(crate) slots: Vec<Option<T>>,
    pub(crate) free: Vec<u32>,
}

impl<T> SlotMap<T> {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    fn insert(&mut self, value: T) -> u32 {
        if let Some(idx) = self.free.pop() {
            self.slots[idx as usize] = Some(value);
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Some(value));
            idx
        }
    }

    fn get(&self, id: u32) -> Option<&T> {
        self.slots.get(id as usize)?.as_ref()
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.slots.get_mut(id as usize)?.as_mut()
    }

    fn remove(&mut self, id: u32) {
        if let Some(slot) = self.slots.get_mut(id as usize) {
            if slot.is_some() {
                *slot = None;
                self.free.push(id);
            }
        }
    }
}

// ── 类型 ─────────────────────────────────────────────────────────────────

type Callback = Box<dyn FnMut() + Send + 'static>;

struct TimerState {
    alive: bool,
    paused: bool,
    remaining: Duration,
    group: Option<u32>,
}

struct WorkerCmd {
    id: u32,
    deadline: Instant,
}
struct RegMsg {
    id: u32,
    group_id: Option<u32>,
    params: TimerParams,
    #[allow(dead_code)]
    deadline: Instant,
}
struct FireEvent {
    id: u32,
}

const SLEEP_CHUNK: Duration = Duration::from_millis(50);

// ── 公开类型 ─────────────────────────────────────────────────────────────

/// 定时器创建参数。
pub struct TimerParams {
    pub delay: Duration,
    pub interval: Option<Duration>,
    pub exec_immediately: bool,
    pub group_id: Option<u32>,
    pub callback: Callback,
}

/// u32 slot 索引，即 timer id。
pub type TimerId = u32;

/// 定时器客户端 —— 可 Clone，面板直接持有。
#[derive(Clone)]
pub struct TimerClient {
    cmd_tx: Sender<WorkerCmd>,
    reg_tx: Sender<RegMsg>,
    state: Arc<Mutex<SlotMap<TimerState>>>,
}

impl TimerClient {
    pub fn create_timer(&self, params: TimerParams) -> TimerId {
        let id = {
            let mut s = self.state.lock().unwrap();
            s.insert(TimerState {
                alive: true,
                paused: false,
                remaining: params.delay,
                group: params.group_id,
            })
        };
        let deadline = Instant::now() + params.delay;
        let group_id = params.group_id;
        let _ = self.reg_tx.send(RegMsg {
            id,
            group_id,
            params,
            deadline,
        });
        let _ = self.cmd_tx.send(WorkerCmd { id, deadline });
        id
    }

    pub fn destroy(&self, id: TimerId) {
        self.state.lock().unwrap().remove(id);
    }

    pub fn pause(&self, id: TimerId) {
        let mut s = self.state.lock().unwrap();
        if let Some(st) = s.get_mut(id) {
            if st.alive && !st.paused {
                st.paused = true;
            }
        }
    }

    pub fn resume(&self, id: TimerId) {
        let remaining = {
            let mut s = self.state.lock().unwrap();
            let st = match s.get_mut(id) {
                Some(st) if st.paused => st,
                _ => return,
            };
            st.paused = false;
            st.remaining
        };
        let _ = self.cmd_tx.send(WorkerCmd {
            id,
            deadline: Instant::now() + remaining,
        });
    }

    pub fn destroy_group(&self, group: u32) {
        let mut s = self.state.lock().unwrap();
        for i in 0..s.slots.len() {
            if let Some(ref st) = s.slots[i] {
                if st.group == Some(group) {
                    s.slots[i] = None;
                    s.free.push(i as u32);
                }
            }
        }
    }
}

/// 主线程定时器服务。
pub struct TimerService {
    pub client: TimerClient,
    fire_rx: Receiver<FireEvent>,
    reg_rx: Receiver<RegMsg>,
    params: SlotMap<TimerParams>,
    groups: HashMap<u32, Vec<u32>>,
}

impl TimerService {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx): (Sender<WorkerCmd>, Receiver<WorkerCmd>) = mpsc::channel();
        let (fire_tx, fire_rx): (Sender<FireEvent>, Receiver<FireEvent>) = mpsc::channel();
        let (reg_tx, reg_rx): (Sender<RegMsg>, Receiver<RegMsg>) = mpsc::channel();
        let state: Arc<Mutex<SlotMap<TimerState>>> = Arc::new(Mutex::new(SlotMap::new()));

        let worker_state = state.clone();
        thread::spawn(move || worker_loop(cmd_rx, fire_tx, worker_state));

        Self {
            client: TimerClient {
                cmd_tx,
                reg_tx,
                state,
            },
            fire_rx,
            reg_rx,
            params: SlotMap::new(),
            groups: HashMap::new(),
        }
    }

    /// 每帧在主线程调用，派发到期回调。
    pub fn tick(&mut self) {
        // 1. 新注册 → 存 params + group 映射 + exec_immediately
        while let Ok(msg) = self.reg_rx.try_recv() {
            let id = msg.id;
            if let Some(g) = msg.group_id {
                self.groups.entry(g).or_default().push(id);
            }
            let exec = msg.params.exec_immediately;
            self.params.insert(msg.params);
            if exec {
                if let Some(p) = self.params.get_mut(id) {
                    (p.callback)();
                }
            }
        }

        // 2. 收集到期事件
        let mut fired = Vec::new();
        while let Ok(ev) = self.fire_rx.try_recv() {
            let alive = self
                .client
                .state
                .lock()
                .map(|s| {
                    s.get(ev.id)
                        .map(|st| st.alive && !st.paused)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if alive {
                fired.push(ev.id);
            }
        }

        // 3. 派发回调 & 重新调度 interval
        for id in fired {
            let ok = self
                .client
                .state
                .lock()
                .map(|s| s.get(id).map(|st| st.alive && !st.paused).unwrap_or(false))
                .unwrap_or(false);
            if !ok {
                continue;
            }

            if let Some(p) = self.params.get_mut(id) {
                (p.callback)();
                if let Some(period) = p.interval {
                    self.client
                        .state
                        .lock()
                        .unwrap()
                        .get_mut(id)
                        .map(|st| st.remaining = period);
                    let _ = self.client.cmd_tx.send(WorkerCmd {
                        id,
                        deadline: Instant::now() + period,
                    });
                } else {
                    self.client.state.lock().unwrap().remove(id);
                    self.params.remove(id);
                }
            }
        }
    }
}

// ── 后台 worker ──────────────────────────────────────────────────────────

fn worker_loop(
    cmd_rx: Receiver<WorkerCmd>,
    fire_tx: Sender<FireEvent>,
    state: Arc<Mutex<SlotMap<TimerState>>>,
) {
    for cmd in cmd_rx {
        let ft = fire_tx.clone();
        let st = state.clone();
        thread::spawn(move || {
            let deadline = cmd.deadline;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                thread::sleep(std::cmp::min(deadline - now, SLEEP_CHUNK));

                let stop = st
                    .lock()
                    .map(|mut s| match s.get_mut(cmd.id) {
                        Some(ts) if !ts.alive => true,
                        Some(ts) if ts.paused => {
                            ts.remaining = deadline.saturating_duration_since(Instant::now());
                            true
                        }
                        Some(_) => false,
                        None => true,
                    })
                    .unwrap_or(true);
                if stop {
                    return;
                }
            }

            let ok = st
                .lock()
                .map(|s| {
                    s.get(cmd.id)
                        .map(|ts| ts.alive && !ts.paused)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if ok {
                let _ = ft.send(FireEvent { id: cmd.id });
            }
        });
    }
}

// ── TimerSet ─────────────────────────────────────────────────────────────

/// TimerClient + 自动 id 缓存 + Drop 时自动批量销毁。
///
/// 面板基类持有此类型，通过 `create_timer` 创建时会自动记录 id，
/// 析构时全部清理。
pub struct TimerSet {
    client: TimerClient,
    ids: Vec<u32>,
}

impl TimerSet {
    pub fn new(client: TimerClient) -> Self {
        Self {
            client,
            ids: Vec::new(),
        }
    }

    pub fn create_timer(&mut self, params: TimerParams) -> u32 {
        let id = self.client.create_timer(params);
        self.ids.push(id);
        id
    }

    pub fn destroy(&mut self, id: u32) {
        self.client.destroy(id);
        self.ids.retain(|&i| i != id);
    }

    pub fn clear(&mut self) {
        for &id in &self.ids {
            self.client.destroy(id);
        }
        self.ids.clear();
    }

    pub fn pause(&self, id: u32) {
        self.client.pause(id);
    }
    pub fn resume(&self, id: u32) {
        self.client.resume(id);
    }
    pub fn inner(&self) -> &TimerClient {
        &self.client
    }
    pub fn inner_mut(&mut self) -> &mut TimerClient {
        &mut self.client
    }
}

impl Drop for TimerSet {
    fn drop(&mut self) {
        self.clear();
    }
}
