//! 2D screen-space particle system —— 雨、雪、火花等 2D 特效的通用模拟器。
//!
//! 粒子在**像素屏幕空间**（原点左上、y 向下，与 `ComputedLayout::rect` 一致）
//! 中模拟，并以 `UiQuad`（plain colored quad）的形式输出。引擎已有的
//! `UiOverlay` 叠加 pass 直接绘制这些四边形——**无需新增/修改任何 shader**。
//!
//! 雨滴本质上就是许多细长、`alpha` 较低的竖向四边形；本模块把它们打包进
//! `UiDrawList::quads`，由 `convert_ui_draw_list_to_overlay` 在帧提取时自动
//! 转成 NDC 并被叠加 pass 消耗。叠加 pass 的提交顺序为：先所有 quad（按
//! `layer` 排序）再所有文本，因此把雨的 `layer` 设为 1，雨就落在黑场
//! （layer 0）之上、标题文本之下。
//!
//! 用法（见 `projects/game/src/intro.rs` 的开屏动画）：
//!
//! ```ignore
//! // 构造时插入资源（雨预设，尺寸随后每帧按 ScreenSize 更新）。
//! app.insert_resource(ParticleSystem2D::rain_preset(1920.0, 1080.0));
//!
//! // 每帧：更新模拟并用 intensity 控制整体可见度，再把粒子追加进 UiDrawList。
//! let quads = {
//!     let rain = world.get_resource_mut::<ParticleSystem2D>().unwrap();
//!     rain.set_bounds(w, h);
//!     rain.intensity = title_alpha; // 随标题淡入淡出
//!     rain.update(dt);
//!     world.get_resource::<ParticleSystem2D>().unwrap().emit_quads()
//! };
//! world.get_resource_mut::<UiDrawList>().unwrap().quads.extend(quads);
//! ```

use super::render::UiQuad;

// ── 单个粒子 ───────────────────────────────────────────────────

/// 一个屏幕空间粒子（像素坐标，y 向下）。
#[derive(Clone, Copy, Debug)]
pub struct Particle2D {
    /// 当前位置（px）。
    pub x: f32,
    /// 当前位置（px，y 向下，故下落时 vy > 0）。
    pub y: f32,
    /// 水平速度（px/s）。
    pub vx: f32,
    /// 垂直速度（px/s，下落为正）。
    pub vy: f32,
    /// 已经存活的时间（s）。
    pub age: f32,
    /// 寿命（s），超过即回收。
    pub life: f32,
    /// 拖尾长度（px）——雨滴为细长条，雪花/火花为 0。
    pub len: f32,
    /// 粗细（px）。
    pub width: f32,
    /// 基础不透明度 0..=1（最终 alpha 还会被系统 `intensity` 与淡入淡出调制）。
    pub alpha: f32,
}

// ── 发射形态 ───────────────────────────────────────────────────

/// 粒子发射形态——影响初始速度与拖尾的朝向。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpawnKind {
    /// 雨：快速下落 + 轻微同向斜风，带竖向拖尾。
    #[default]
    Rain,
    /// 雪：缓慢飘落 + 正弦摆动，无拖尾。
    Snow,
    /// 火花：随机四散 + 重力，无拖尾。
    Spark,
}

// ── 粒子系统 ───────────────────────────────────────────────────

/// 一个 2D 屏幕空间粒子系统（作为 ECS 资源存在）。
///
/// 配置字段全为 `pub`，调用方（如开屏动画）可随时调整 `intensity`、
/// `color`、`spawn_rate` 等来控制表现。`update` 推进模拟，`emit_quads`
/// 输出待绘制的 `UiQuad` 列表。
#[derive(Debug)]
pub struct ParticleSystem2D {
    /// 存活粒子池（无上限但有 `MAX_PARTICLES` 软上限防止失控）。
    pub particles: Vec<Particle2D>,
    /// 发射速率（粒子/秒）。实际发射数会再乘以 `intensity`。
    pub spawn_rate: f32,
    /// 重力加速度（px/s²，向下为正）。
    pub gravity: f32,
    /// 水平风偏（px/s²，向右为正），决定雨的倾斜角。
    pub wind: f32,
    /// 初始垂直速度范围（px/s）。
    pub speed_min: f32,
    pub speed_max: f32,
    /// 拖尾长度范围（px）。
    pub len_min: f32,
    pub len_max: f32,
    /// 粗细范围（px）。
    pub width_min: f32,
    pub width_max: f32,
    /// 颜色（RGB，0..=1）。
    pub color: [f32; 3],
    /// 基础不透明度范围 0..=1。
    pub alpha_min: f32,
    pub alpha_max: f32,
    /// 出生 y 范围 `[-spawn_top, 0]`（屏幕上方一点，平滑入场）。
    pub spawn_top: f32,
    /// 整体强度 0..=1：同时调制发射速率与最终 alpha。设 0 即完全停雨。
    pub intensity: f32,
    /// 发射形态。
    pub kind: SpawnKind,
    /// 当前屏幕宽（px），每帧由 `set_bounds` 更新。
    pub width: f32,
    /// 当前屏幕高（px），每帧由 `set_bounds` 更新。
    pub height: f32,
    /// 软上限：粒子数超过此值则暂停发射（保护顶点缓冲与性能）。
    pub max_particles: usize,
    /// 雪花的水平摆动幅度（px/s），仅 `Snow` 用。
    pub sway: f32,
    rng: u64,
    spawn_accum: f32,
}

impl ParticleSystem2D {
    /// 构造一个通用粒子系统（默认雨形态）。具体表现由调用方配置。
    pub fn new() -> Self {
        Self::rain_preset(1920.0, 1080.0)
    }

    /// 雨滴预设——覆盖整屏的中等密度斜雨，冷蓝白色。
    ///
    /// `width`/`height` 仅作初始屏幕尺寸，运行时用 `set_bounds` 按
    /// `ScreenSize` 实时更新即可。
    pub fn rain_preset(width: f32, height: f32) -> Self {
        let area_scale = (width / 1920.0).clamp(0.5, 2.0);
        Self {
            particles: Vec::with_capacity(1024),
            spawn_rate: 420.0 * area_scale,
            gravity: 240.0,
            wind: 90.0,
            speed_min: 900.0,
            speed_max: 1500.0,
            len_min: 16.0,
            len_max: 42.0,
            width_min: 1.0,
            width_max: 2.4,
            color: [0.72, 0.82, 0.96],
            alpha_min: 0.16,
            alpha_max: 0.42,
            spawn_top: 70.0,
            intensity: 1.0,
            kind: SpawnKind::Rain,
            width,
            height,
            max_particles: (900.0 * area_scale) as usize,
            sway: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            spawn_accum: 0.0,
        }
    }

    /// 雪花预设——缓慢飘落、左右摆动，无拖尾。
    pub fn snow_preset(width: f32, height: f32) -> Self {
        let area_scale = (width / 1920.0).clamp(0.5, 2.0);
        Self {
            particles: Vec::with_capacity(1024),
            spawn_rate: 120.0 * area_scale,
            gravity: 60.0,
            wind: 0.0,
            speed_min: 40.0,
            speed_max: 110.0,
            len_min: 0.0,
            len_max: 0.0,
            width_min: 1.5,
            width_max: 3.5,
            color: [0.95, 0.97, 1.0],
            alpha_min: 0.5,
            alpha_max: 0.9,
            spawn_top: 40.0,
            intensity: 1.0,
            kind: SpawnKind::Snow,
            width,
            height,
            max_particles: (400.0 * area_scale) as usize,
            sway: 40.0,
            rng: 0x1B87_3D5E_9C2A_4F11,
            spawn_accum: 0.0,
        }
    }

    /// 更新当前屏幕尺寸（通常每帧从 `ScreenSize` 资源读取后调用）。
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    /// 每帧推进模拟 `dt` 秒：按 `intensity` 发射新粒子，积分运动并回收离场粒子。
    pub fn update(&mut self, dt: f32) {
        // 1. 发射（受 intensity 调制；intensity 为 0 则不发射，现有粒子自然消亡）。
        self.spawn_accum += self.spawn_rate * self.intensity * dt;
        let mut to_spawn = self.spawn_accum.floor() as i32;
        self.spawn_accum -= to_spawn as f32;
        while to_spawn > 0 && self.particles.len() < self.max_particles {
            self.spawn_one();
            to_spawn -= 1;
        }

        // 2. 积分运动 + 回收（离场或寿终）。
        //    先把要用到的标量提到局部变量，避免闭包整体捕获 `self`
        //    （与 `self.particles` 的可变借用冲突）。
        let h = self.height;
        let w = self.width;
        let gravity = self.gravity;
        let kind = self.kind;
        let sway = self.sway;
        self.particles.retain_mut(|p| {
            p.vy += gravity * dt;
            if kind == SpawnKind::Snow {
                // 正弦摆动由 sway 幅值 + 相位（用 age 近似）驱动。
                p.vx = (p.age * 2.2).sin() * sway;
            }
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;
            p.age < p.life && p.y < h + 60.0 && p.x > -60.0 && p.x < w + 60.0
        });
    }

    /// 发射一个符合当前形态配置的粒子。
    fn spawn_one(&mut self) {
        let w = self.width;
        let x = Self::rand(&mut self.rng) * w;
        let y = -self.spawn_top * Self::rand(&mut self.rng);
        let life = match self.kind {
            SpawnKind::Rain => 6.0,
            SpawnKind::Snow => 12.0,
            SpawnKind::Spark => 1.2,
        };
        let speed = lerp(self.speed_min, self.speed_max, Self::rand(&mut self.rng));
        let (vx, len) = match self.kind {
            SpawnKind::Rain => (self.wind * (0.4 + 0.6 * Self::rand(&mut self.rng)), lerp(self.len_min, self.len_max, Self::rand(&mut self.rng))),
            SpawnKind::Snow => (0.0, 0.0),
            SpawnKind::Spark => {
                let ang = Self::rand(&mut self.rng) * std::f32::consts::TAU;
                let sp = lerp(self.speed_min, self.speed_max, Self::rand(&mut self.rng));
                (ang.cos() * sp, 0.0)
            }
        };
        let width = lerp(self.width_min, self.width_max, Self::rand(&mut self.rng));
        let alpha = lerp(self.alpha_min, self.alpha_max, Self::rand(&mut self.rng));
        self.particles.push(Particle2D {
            x,
            y,
            vx,
            vy: speed,
            age: 0.0,
            life,
            len,
            width,
            alpha,
        });
    }

    /// 输出当前存活粒子对应的 `UiQuad` 列表（像素坐标矩形，可直接追加进
    /// `UiDrawList::quads`）。`layer` 固定为 1，绘制在黑场之上、文本之下。
    pub fn emit_quads(&self) -> Vec<UiQuad> {
        let h = self.height;
        let mut quads = Vec::with_capacity(self.particles.len());
        for p in &self.particles {
            // 出生淡入（前 0.18s）避免凭空出现。
            let fade_in = (p.age / 0.18).min(1.0);
            // 接近底部时淡出，避免硬切出屏幕。
            let bottom_fade = if p.y > h - 50.0 {
                ((h + 50.0 - p.y) / 100.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let a = p.alpha * self.intensity * fade_in * bottom_fade;
            if a <= 0.002 {
                continue;
            }
            // 拖尾向上延伸（y 向下，故顶端 = y - len）。
            let rect = [p.x - p.width * 0.5, p.y - p.len, p.width, p.len.max(p.width)];
            quads.push(UiQuad {
                rect,
                color: [self.color[0], self.color[1], self.color[2], a],
                border_radius: p.width * 0.5, // 胶囊状端点
                layer: 1,
            });
        }
        quads
    }
}

impl ParticleSystem2D {
    /// xorshift64 PRNG（避免引入 rand 依赖）。返回 64 位无符号整数。
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// `[0,1)` 均匀分布伪随机（xorshift64，避免引入 rand 依赖）。
    fn rand(state: &mut u64) -> f32 {
        (Self::xorshift(state) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// 线性插值。
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
