//! 开场 intro screen：黑场淡出 → 标题上浮 → 停留 → 淡出，任意键/点击跳过。
//!
//! 纯 ECS UI：main 里调用 [`spawn_ui`] 生成 UI 实体（全屏黑场 + 标题/副标题/
//! 提示三个文本实体），[`advance`] system 每帧推进 keyframe 动画并写回
//! `Style` / `Text` 组件——布局由 `ui::layout` 系统消费，绘制由
//! `ui::render` → overlay pass 消费，本模块不碰任何 egui。
//!
//! 动画值全部由 keyframe `AnimationSequence<f32>` 驱动——每条属性一条
//! 时间轴（`keyframes!` 宏），一条曲线覆盖整个 intro（淡入→停留→淡出）；
//! 跳过 = `advance_to(duration)` 直接快进到结尾。

use keyframe::functions::{EaseInCubic, EaseInOutCubic, EaseOutCubic};
use keyframe::{keyframes, AnimationSequence};
use prism_ecs::{Entity, World};
use prism_engine::ui::{
    Anchors, ComputedLayout, Margin, Node, Pivot, Style, Text, TextAlign, UiInputState,
};

/// intro 参数。
#[derive(Clone, Debug)]
pub struct IntroConfig {
    /// 主标题。
    pub title: String,
    /// 副标题。
    pub subtitle: String,
    /// 底部跳过提示。
    pub prompt: String,
    /// 黑场淡出 + 标题上浮时长（秒）。
    pub fade_in: f32,
    /// 标题停留时长（秒）。
    pub hold: f32,
    /// 标题淡出时长（秒）。
    pub fade_out: f32,
}

impl Default for IntroConfig {
    fn default() -> Self {
        Self {
            title: "PrismaRev".into(),
            subtitle: "Vulkan 实时渲染引擎".into(),
            prompt: "按任意键或点击继续".into(),
            fade_in: 0.9,
            hold: 1.6,
            fade_out: 0.6,
        }
    }
}

/// 标题初始位置（margin.top，CENTER 锚点下负值 = 中心上方）。
const TITLE_BASE_MARGIN: f32 = -16.0;
/// 副标题相对标题的纵向间距（px）。
const SUBTITLE_OFFSET: f32 = 64.0;
/// 跳过提示离底部的距离（px）。
const PROMPT_BOTTOM_MARGIN: f32 = -48.0;

/// 生成 intro 的 UI 实体并返回驱动状态（作为 World resource 插入）。
pub fn spawn_ui(world: &mut World, cfg: IntroConfig) -> IntroState {
    // 全屏黑场：STRETCH 锚点 + 纯黑背景。
    let overlay_entity = world.spawn();
    world.insert(overlay_entity, Node);
    world.insert(
        overlay_entity,
        Style {
            background: [0.0, 0.0, 0.0, 1.0],
            ..Default::default()
        },
    );
    world.insert(overlay_entity, ComputedLayout::default());

    // 三个文本实体：固定宽度容器 + CENTER/BOTTOM_CENTER 锚点。
    let title_entity = spawn_text(
        world,
        cfg.title.clone(),
        84.0,
        [232.0 / 255.0, 236.0 / 255.0, 244.0 / 255.0, 1.0],
        TITLE_BASE_MARGIN,
        Anchors::CENTER,
    );
    let subtitle_entity = spawn_text(
        world,
        cfg.subtitle.clone(),
        18.0,
        [154.0 / 255.0, 166.0 / 255.0, 196.0 / 255.0, 1.0],
        TITLE_BASE_MARGIN + SUBTITLE_OFFSET,
        Anchors::CENTER,
    );
    let prompt_entity = spawn_text(
        world,
        cfg.prompt.clone(),
        14.0,
        [154.0 / 255.0, 166.0 / 255.0, 196.0 / 255.0, 1.0],
        PROMPT_BOTTOM_MARGIN,
        Anchors::BOTTOM_CENTER,
    );

    let (fade_in, hold, fade_out) = (cfg.fade_in, cfg.hold, cfg.fade_out);
    let hold_end = fade_in + hold;

    // 每条属性一条时间轴：淡入 → 停留 → 淡出 的完整曲线。
    let overlay = keyframes![
        (1.0, 0.0, EaseInOutCubic),
        (0.0, fade_in),
        (0.0, hold_end + fade_out)
    ];
    let title_opacity = keyframes![
        (0.0, 0.0, EaseOutCubic),
        (1.0, fade_in),
        (1.0, hold_end, EaseInCubic),
        (0.0, hold_end + fade_out)
    ];
    let title_y = keyframes![
        (24.0, 0.0, EaseOutCubic),
        (0.0, fade_in),
        (0.0, hold_end, EaseInCubic),
        (-24.0, hold_end + fade_out)
    ];

    IntroState {
        overlay,
        title_opacity,
        title_y,
        skip: false,
        overlay_entity,
        title_entity,
        subtitle_entity,
        prompt_entity,
    }
}

/// 生成一个文本 UI 实体：固定宽度容器 + 指定锚点/边距。
fn spawn_text(
    world: &mut World,
    content: String,
    font_size: f32,
    color: [f32; 4],
    top_margin: f32,
    anchors: Anchors,
) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node);
    world.insert(
        entity,
        Style {
            width: Some(900.0),
            height: Some(font_size + 24.0),
            anchors,
            pivot: Pivot::CENTER,
            margin: Margin {
                top: top_margin,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    world.insert(entity, ComputedLayout::default());
    world.insert(
        entity,
        Text {
            content,
            font_size,
            color,
            alignment: TextAlign::Center,
        },
    );
    entity
}

/// intro 状态（ECS World 资源）——持有动画时间轴 + 受控 UI 实体。
pub struct IntroState {
    /// 全屏黑场 alpha（1 = 全黑）。
    overlay: AnimationSequence<f32>,
    /// 标题不透明度。
    title_opacity: AnimationSequence<f32>,
    /// 标题纵向位移（px，>0 在下方，动画归零上浮）。
    title_y: AnimationSequence<f32>,
    /// 跳过请求（`advance` 里置位并消费）。
    skip: bool,
    overlay_entity: Entity,
    title_entity: Entity,
    subtitle_entity: Entity,
    prompt_entity: Entity,
}

impl IntroState {
    /// 推进 intro 动画（跳过时快进到结尾）。
    fn tick(&mut self, dt: f32) {
        if self.skip {
            self.skip = false;
            // 跳过：快进到时间轴结尾，intro 直接结束。
            self.overlay.advance_to(self.overlay.duration());
            self.title_opacity.advance_to(self.title_opacity.duration());
            self.title_y.advance_to(self.title_y.duration());
            return;
        }
        if self.overlay.finished() {
            return;
        }
        self.overlay.advance_by(dt as f64);
        self.title_opacity.advance_by(dt as f64);
        self.title_y.advance_by(dt as f64);
    }
}

/// ECS system：每帧推进 intro 动画并写回 UI 组件。
///
/// 跳过检测读 [`UiInputState`]（引擎每帧插入的输入快照）——任意键或点击。
pub fn advance(world: &mut World, dt: f32) {
    // 1. 跳过检测（不可变读输入快照）。
    let skip = world
        .get_resource::<UiInputState>()
        .is_some_and(|i| {
            i.left_clicked
                || i.pressed_keys.iter().any(|k| {
                    matches!(
                        k,
                        prism_engine::input::KeyCode::Space | prism_engine::input::KeyCode::Enter
                    )
                })
        });

    // 2. 推进动画、取出本帧要写回的动画值与受控实体（借用在此结束）。
    let (overlay_alpha, title_alpha, title_y, prompt_alpha, overlay_e, title_e, subtitle_e, prompt_e) = {
        let Some(state) = world.get_resource_mut::<IntroState>() else {
            return;
        };
        if skip {
            state.skip = true;
        }
        state.tick(dt);
        let title_alpha = state.title_opacity.now();
        // 提示呼吸：停留阶段跳动，透明度跟随标题。
        let time = state.overlay.time() as f32;
        let pulse = 0.35 + 0.35 * ((time * 4.0).sin() * 0.5 + 0.5);
        (
            state.overlay.now(),
            title_alpha,
            state.title_y.now(),
            title_alpha * pulse,
            state.overlay_entity,
            state.title_entity,
            state.subtitle_entity,
            state.prompt_entity,
        )
    };

    // 3. 写回组件（Style 背景/边距 + Text 颜色）。
    if let Some(style) = world.get_mut::<Style>(overlay_e) {
        style.background[3] = overlay_alpha;
    }
    if let Some(style) = world.get_mut::<Style>(title_e) {
        style.margin.top = TITLE_BASE_MARGIN + title_y;
    }
    if let Some(text) = world.get_mut::<Text>(title_e) {
        text.color[3] = title_alpha;
    }
    if let Some(style) = world.get_mut::<Style>(subtitle_e) {
        style.margin.top = TITLE_BASE_MARGIN + SUBTITLE_OFFSET + title_y;
    }
    if let Some(text) = world.get_mut::<Text>(subtitle_e) {
        text.color[3] = title_alpha;
    }
    if let Some(text) = world.get_mut::<Text>(prompt_e) {
        text.color[3] = prompt_alpha;
    }
}
