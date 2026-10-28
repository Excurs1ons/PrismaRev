//! Cozy Fishing 库目标。
//!
//! 桌面端由 `src/main.rs` 调用 [`build_app`]；Android 端由
//! [`android_main`] 导出 JNI 入口（产出 `libfishing.so`）。

pub mod game;

/// 构造应用：引擎从 `assets/scenes.pak` 加载湖面场景，本项目注册玩法 system。
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);

    app.add_system("game::world", game::world::tick);
    app.add_system("game::fishing", game::fishing::tick)
        .add_system("game::boat", boat_system);
    app
}

/// `boat::tick` 的 Player 适配器。
///
/// `BoatState` 是项目私有类型：这里临时放进 ECS resource，交给与插件形态
/// 相同的 `tick`，再放回去。引擎只认自己的通用 resource 机制。
fn boat_system(world: &mut prism_engine::ecs::World, dt: f32) {
    let mut state = world.remove_resource::<game::boat::BoatState>();
    {
        let mut api = prism_game_api::GameWorld::host(world);
        game::boat::tick(&mut state, &mut api, dt);
    }
    if let Some(state) = state {
        world.insert_resource(state);
    }
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。
///
/// GameActivity 的原生 glue 通过 `dlsym` 解析该符号，因此必须保持 C 链接
/// 与未修饰的名字。
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
