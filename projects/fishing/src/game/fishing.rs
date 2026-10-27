//! 钓鱼状态机（M0 垂直切片）。
//!
//! 这是**用户项目**的玩法层：引擎只提供通用的 `LineRenderer` /
//! `SpriteRenderer` / `UiInputState`，鱼竿、浮标、咬钩时序全在这里。
//!
//! 状态流转：
//!
//! ```text
//! Idle ──点击──> Casting ──落水──> Waiting ──计时──> Biting
//!   ^                                                  │
//!   └────── Caught <──收线成功── Reeling <──点击────────┘
//!                                                      └──超时──> Waiting
//! ```

use prism_engine::ecs::{Entity, World};
use prism_engine::scene::components::{LineRenderer, LocalTransform, Name, SpriteRenderer};
use prism_engine::ui::UiInputState;

/// 鱼竿尖端相对船锚点的偏移（船体局部像素）。
const ROD_OFFSET: [f32; 2] = [44.0, 24.0];
/// 抛竿落点相对船的水平距离。
const CAST_DISTANCE: f32 = 150.0;
/// 浮标飞行时间。
const CAST_SECONDS: f32 = 0.55;
/// 咬钩提示持续时间（超过就脱钩）。
const BITE_WINDOW: f32 = 1.1;
/// 收线动画时间。
const REEL_SECONDS: f32 = 0.9;
/// 水面高度（世界 y）。
const WATER_LINE: f32 = 80.0;
/// 鱼线排序层。
const LINE_LAYER: i32 = 45;

/// 钓鱼阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Casting,
    Waiting,
    Biting,
    Reeling,
    Caught,
}

impl Default for Phase {
    fn default() -> Self {
        Self::Idle
    }
}

/// 钓鱼状态（ECS 资源）。
#[derive(Default)]
pub struct FishingState {
    phase: Phase,
    /// 当前阶段已经过的时间（秒）。
    elapsed: f32,
    /// 本阶段的目标时长；`0` 表示按逻辑自行判定。
    duration: f32,
    /// 浮标当前世界位置。
    bobber_pos: [f32; 2],
    /// 抛竿起点（船锚点）。
    cast_origin: [f32; 2],
    /// 本次等待的咬钩时间（伪随机）。
    bite_at: f32,
    /// 已钓到的鱼数（M0 只做计数）。
    caught: u32,
    boat: Option<Entity>,
    bobber: Option<Entity>,
    line: Option<Entity>,
}

impl FishingState {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn caught(&self) -> u32 {
        self.caught
    }
}

/// 每帧推进钓鱼状态机。
pub fn tick(world: &mut World, dt: f32) {
    if world.get_resource::<FishingState>().is_none() {
        let mut state = FishingState::default();
        for (entity, name) in world.query::<Name>() {
            match name.0.as_str() {
                "boat" => state.boat = Some(entity),
                "bobber" => state.bobber = Some(entity),
                _ => {}
            }
        }
        state.line = spawn_line(world);
        world.insert_resource(state);
    }

    let Some(mut state) = world.remove_resource::<FishingState>() else {
        return;
    };

    // --- 输入（引擎每帧在调度前写入的 UI 输入快照） ---
    let clicked = world
        .get_resource::<UiInputState>()
        .map(|input| input.left_clicked)
        .unwrap_or(false);
    let pressed_space = world
        .get_resource::<UiInputState>()
        .map(|input| {
            input
                .pressed_keys
                .contains(&prism_engine::input::KeyCode::Space)
        })
        .unwrap_or(false);
    // `FISHING_AUTOPLAY=1` 让状态机自己推进：用于演示与自动化截图，
    // 正常游玩时不设这个变量，完全由输入驱动。
    let autoplay = std::env::var_os("FISHING_AUTOPLAY").is_some();
    let auto_fire =
        autoplay && matches!(state.phase, Phase::Idle | Phase::Biting) && state.elapsed > 1.0;
    let action = clicked || pressed_space || auto_fire;
    // --- 船锚点 ---
    let boat_pos = state
        .boat
        .and_then(|e| world.get::<LocalTransform>(e))
        .map(|t| [t.position.x, t.position.y])
        .unwrap_or([0.0, WATER_LINE]);

    state.elapsed += dt;

    match state.phase {
        Phase::Idle => {
            if action {
                state.phase = Phase::Casting;
                state.elapsed = 0.0;
                state.duration = CAST_SECONDS;
                state.cast_origin = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
                state.bobber_pos = state.cast_origin;
                log::info!("fishing: cast");
            }
        }
        Phase::Casting => {
            let t = (state.elapsed / CAST_SECONDS).clamp(0.0, 1.0);
            let target = [boat_pos[0] + CAST_DISTANCE, WATER_LINE];
            // 抛物线：水平线性插值，垂直方向加一个拱形。
            let arc = (t * std::f32::consts::PI).sin() * 60.0;
            state.bobber_pos = [
                state.cast_origin[0] + (target[0] - state.cast_origin[0]) * t,
                state.cast_origin[1] + (target[1] - state.cast_origin[1]) * t + arc,
            ];
            if t >= 1.0 {
                state.phase = Phase::Waiting;
                state.elapsed = 0.0;
                state.bite_at = 1.4 + pseudo_random(state.caught) * 2.2;
                log::info!("fishing: bobber landed, waiting {:.2}s", state.bite_at);
            }
        }
        Phase::Waiting => {
            state.bobber_pos = [
                boat_pos[0] + CAST_DISTANCE,
                WATER_LINE + (state.elapsed * 2.2).sin() * 1.5,
            ];
            if state.elapsed >= state.bite_at {
                state.phase = Phase::Biting;
                state.elapsed = 0.0;
                log::info!("fishing: bite!");
            }
        }
        Phase::Biting => {
            // 咬钩时浮标下沉抖动。
            state.bobber_pos = [
                boat_pos[0] + CAST_DISTANCE,
                WATER_LINE - 4.0 + (state.elapsed * 18.0).sin() * 2.5,
            ];
            if action {
                state.phase = Phase::Reeling;
                state.elapsed = 0.0;
                log::info!("fishing: hook set");
            } else if state.elapsed >= BITE_WINDOW {
                state.phase = Phase::Waiting;
                state.elapsed = 0.0;
                state.bite_at = 1.2 + pseudo_random(state.caught + 7) * 2.0;
                log::info!("fishing: missed the bite");
            }
        }
        Phase::Reeling => {
            let t = (state.elapsed / REEL_SECONDS).clamp(0.0, 1.0);
            let target = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            state.bobber_pos = [
                state.bobber_pos[0] + (target[0] - state.bobber_pos[0]) * t * 0.35,
                state.bobber_pos[1] + (target[1] - state.bobber_pos[1]) * t * 0.35,
            ];
            if t >= 1.0 {
                state.caught += 1;
                state.phase = Phase::Caught;
                state.elapsed = 0.0;
                log::info!("fishing: landed fish #{}", state.caught);
            }
        }
        Phase::Caught => {
            if state.elapsed >= 0.8 {
                state.phase = Phase::Idle;
                state.elapsed = 0.0;
                state.bobber_pos = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            }
        }
    }

    // --- 写回浮标位置与表现 ---
    if let Some(bobber) = state.bobber {
        if let Some(transform) = world.get_mut::<LocalTransform>(bobber) {
            transform.position = glam::Vec3::new(state.bobber_pos[0], state.bobber_pos[1], 0.0);
        }
        if let Some(sprite) = world.get_mut::<SpriteRenderer>(bobber) {
            // 咬钩时闪一下暖色，让玩家看清窗口。
            sprite.color = if state.phase == Phase::Biting {
                [1.0, 0.55, 0.35, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            sprite.sorting_layer = if state.phase == Phase::Idle { -100 } else { 60 };
        }
    }

    // --- 鱼线：竿尖 → 浮标（中间一点下垂） ---
    if let Some(line) = state.line {
        if let Some(renderer) = world.get_mut::<LineRenderer>(line) {
            let tip = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            let visible = state.phase != Phase::Idle && state.phase != Phase::Caught;
            renderer.points = if visible {
                let mid = [
                    (tip[0] + state.bobber_pos[0]) * 0.5,
                    (tip[1] + state.bobber_pos[1]) * 0.5 - 6.0,
                    0.0,
                ];
                vec![
                    [tip[0], tip[1], 0.0],
                    mid,
                    [state.bobber_pos[0], state.bobber_pos[1], 0.0],
                ]
            } else {
                Vec::new()
            };
        }
    }

    world.insert_resource(state);
}

fn spawn_line(world: &mut World) -> Option<Entity> {
    let entity = world.spawn();
    world.insert(entity, Name("fishing-line".to_string()));
    world.insert(entity, LocalTransform::default());
    world.insert(
        entity,
        LineRenderer {
            points: Vec::new(),
            color: [0.92, 0.95, 1.0, 0.85],
            width: 0.6,
            sorting_layer: LINE_LAYER,
            order_in_layer: 0,
        },
    );
    Some(entity)
}

/// 稳定的伪随机（0..1），避免为一个演示引入 rand 依赖。
fn pseudo_random(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x & 0xFFFF) as f32 / 65535.0
}
