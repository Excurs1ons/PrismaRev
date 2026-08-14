//! 开场 intro screen：黑场淡出 → 标题上浮 → 停留 → 淡出，任意键/点击跳过。
//!
//! 实体由 `assets/scenes/intro.scene.json` 加载（4 个 UI 实体：黑场 + 标题 +
//! 副标题 + 提示）。[`advance`] system 每帧按 `Name` 组件查找实体，推进
//! keyframe 动画并写回 `Style` / `Text` 组件。

use keyframe::functions::{EaseInCubic, EaseInOutCubic, EaseOutCubic};
use keyframe::{keyframes, AnimationSequence};
use prism_ecs::{Entity, World};
use prism_engine::scene::components::Name;
use prism_engine::ui::{ParticleSystem2D, ScreenSize, Style, Text, UiDrawList, UiInputState};

/// 标题初始位置（margin.top，CENTER 锚点下负值 = 中心上方）。
const TITLE_BASE_MARGIN: f32 = -16.0;
/// 副标题相对标题的纵向间距（px）。
const SUBTITLE_OFFSET: f32 = 64.0;
/// 跳过提示离底部的距离（px）。

/// 各实体的 `Name` 组件值（与 intro.scene.json 一致）。
const ENTITY_OVERLAY: &str = "intro_overlay";
const ENTITY_TITLE: &str = "intro_title";
const ENTITY_SUBTITLE: &str = "intro_subtitle";
const ENTITY_PROMPT: &str = "intro_prompt";

/// intro 动画状态（ECS World 资源）——持有动画时间轴 + 受控 UI 实体。
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

/// 在第一帧按 Name 查找场景实体，构造 IntroState。
fn initialize_intro(world: &mut World) -> Option<IntroState> {
    let mut entities: std::collections::HashMap<&str, Entity> = std::collections::HashMap::new();

    for (entity, name) in world.query::<Name>() {
        match name.0.as_str() {
            ENTITY_OVERLAY => { entities.insert(ENTITY_OVERLAY, entity); }
            ENTITY_TITLE => { entities.insert(ENTITY_TITLE, entity); }
            ENTITY_SUBTITLE => { entities.insert(ENTITY_SUBTITLE, entity); }
            ENTITY_PROMPT => { entities.insert(ENTITY_PROMPT, entity); }
            _ => {}
        }
    }

    let overlay_entity = *entities.get(ENTITY_OVERLAY)?;
    let title_entity = *entities.get(ENTITY_TITLE)?;
    let subtitle_entity = *entities.get(ENTITY_SUBTITLE)?;
    let prompt_entity = *entities.get(ENTITY_PROMPT)?;

    log::info!("intro: found all 4 UI entities");

    let fade_in = 0.9;
    let hold = 1.6;
    let fade_out = 0.6;
    let hold_end = fade_in + hold;

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

    Some(IntroState {
        overlay,
        title_opacity,
        title_y,
        skip: false,
        overlay_entity,
        title_entity,
        subtitle_entity,
        prompt_entity,
    })
}

/// ECS system：每帧推进 intro 动画并写回 UI 组件。
///
/// 第一帧按 `Name` 组件查找实体并创建 [`IntroState`] 作为 ECS resource。
/// 跳过检测读 [`UiInputState`]（引擎每帧插入的输入快照）——任意键或点击。
pub fn advance(world: &mut World, dt: f32) {
    // 1. 惰性初始化：第一帧按 Name 查找实体。
    if world.get_resource::<IntroState>().is_none() {
        if let Some(state) = initialize_intro(world) {
            world.insert_resource(state);
        } else {
            // 场景实体尚未就绪，等待下一帧。
            return;
        }
    }

    // 2. 跳过检测。
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

    // 3. 推进动画、取出本帧要写回的动画值与受控实体。
    let (overlay_alpha, title_alpha, title_y, prompt_alpha, overlay_e, title_e, subtitle_e, prompt_e) = {
        let Some(state) = world.get_resource_mut::<IntroState>() else {
            return;
        };
        if skip {
            state.skip = true;
        }
        state.tick(dt);
        let title_alpha = state.title_opacity.now();
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

    // 4. 开屏雨滴（2D 粒子系统）：随标题淡入淡出，追加进 UI 绘制列表。
    //    ui::render 已在 advance 之前生成本帧的 UiDrawList，这里只是扩展其
    //    quads；随后 convert_ui_draw_list_to_overlay 会自动把它纳入叠加 pass。
    {
        let (w, h) = world
            .get_resource::<ScreenSize>()
            .map(|s| (s.width, s.height))
            .unwrap_or((1920.0, 1080.0));

        // 更新模拟（先释放 ScreenSize 只读借用，再取可变借用）。
        if let Some(rain) = world.get_resource_mut::<ParticleSystem2D>() {
            rain.set_bounds(w, h);
            rain.intensity = title_alpha; // 雨随标题一起淡入/淡出
            rain.update(dt);
        }

        // 取出本帧粒子四边形（owned，离开 get_resource 作用域即释放借用），
        // 再追加进 UiDrawList，避免与上面的只读/可变借用产生冲突。
        let rain_quads = world
            .get_resource::<ParticleSystem2D>()
            .map(|r| r.emit_quads());
        if let Some(quads) = rain_quads {
            if let Some(dl) = world.get_resource_mut::<UiDrawList>() {
                dl.quads.extend(quads);
            }
        }
    }

    // 5. 写回组件。
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