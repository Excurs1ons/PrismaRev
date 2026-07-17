//! Application main loop.
//!
//! Implements winit's [`ApplicationHandler`] trait. On startup it opens a
//! window, builds a [`Renderer`], creates an ECS [`World`] with a test scene
//! of three cubes, and drives [`render_system`] each frame.
//!
//! Input events are routed to [`InputState`], and [`OrbitCameraController`]
//! reads the input state to update the [`OrbitCamera`] every frame.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::KeyCode;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_ecs::World;
use prism_render::{DebugMode, FrameUBOData, NormalSpace, OverlayAction, Renderer, Vertex};

use crate::camera::OrbitCamera;
use crate::camera_controller::OrbitCameraController;
use crate::input::{InputState, MouseButton};
use crate::render_system::{render_system, MeshHandle, MeshManager, PbrMaterial, Transform};

/// Locate and read the equirectangular HDR environment map for image-based
/// lighting. Scans the `assets/` directory (and exe-relative variants) for any
/// `*.hdr` file — so the resource can keep its own name (e.g.
/// `valley_of_desolation_1k.hdr`) instead of being renamed. An explicit
/// `env.hdr` is preferred if present; otherwise the first `.hdr` (in
/// deterministic order) is used. Returns `None` if no file is found — the
/// renderer then uses a procedural fallback environment.
fn load_env_bytes() -> Option<Vec<u8>> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("assets")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("assets"));
            dirs.push(dir.join("../../../assets"));
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_hdr = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("hdr"))
                    .unwrap_or(false);
                if is_hdr {
                    candidates.push(path);
                }
            }
        }
    }

    // Prefer an explicit "env.hdr"; otherwise use the first .hdr found.
    candidates.sort();
    if let Some(idx) = candidates.iter().position(|p| {
        p.file_name()
            .map(|n| n.eq_ignore_ascii_case("env.hdr"))
            .unwrap_or(false)
    }) {
        candidates.swap(0, idx);
    }

    for path in &candidates {
        match std::fs::read(path) {
            Ok(bytes) => {
                log::info!("loaded environment map: {}", path.display());
                return Some(bytes);
            }
            Err(e) => log::warn!("failed to read {}: {e}", path.display()),
        }
    }
    log::info!("no environment map found in assets/; using procedural fallback");
    None
}

// ---------------------------------------------------------------------------
// Cube geometry (24 vertices, 36 indices)
// ---------------------------------------------------------------------------

/// Build the default demo scene: a sphere on the left and two cubes on the
/// center/right, each as an ECS entity referencing a GPU mesh via
/// [`MeshHandle`]. Returns the populated world and the mesh owner.
fn create_test_scene(renderer: &Renderer) -> (World, MeshManager) {
    let cube_mesh = renderer
        .create_mesh(&cube_vertices(), Some(&cube_indices()))
        .expect("create cube mesh");
    let (sphere_verts, sphere_idx) = sphere_mesh(32, 24);
    let sphere_mesh = renderer
        .create_mesh(&sphere_verts, Some(&sphere_idx))
        .expect("create sphere mesh");

    let mut world = World::new();
    // Left: sphere, center: PBR cube, right: cube.
    let configs = [
        ([-2.5, 0.0, 0.0], 0usize, false),
        ([0.0, 0.0, 0.0], 1, true), // middle -> PBR + IBL
        ([2.5, 0.0, 0.0], 1, false),
    ];
    for &(pos, mesh_idx, pbr) in &configs {
        let entity = world.spawn();
        world.insert(
            entity,
            Transform {
                translation: pos,
                ..Default::default()
            },
        );
        world.insert(entity, MeshHandle(mesh_idx));
        if pbr {
            world.insert(entity, PbrMaterial::default());
        }
    }

    let mut mesh_manager = MeshManager::new();
    mesh_manager.add(sphere_mesh);
    mesh_manager.add(cube_mesh);
    (world, mesh_manager)
}

fn cube_vertices() -> Vec<Vertex> {
    // Each face: 4 corners with that face's normal, each gets a face color.
    let colors: [[f32; 3]; 6] = [
        [1.0, 0.2, 0.2], // front:  red
        [0.2, 1.0, 0.2], // back:   green
        [0.2, 0.2, 1.0], // right:  blue
        [1.0, 1.0, 0.2], // left:   yellow
        [0.2, 1.0, 1.0], // top:    cyan
        [1.0, 0.2, 1.0], // bottom: magenta
    ];
    let (positions, normals): ([[f32; 3]; 24], [[f32; 3]; 24]) = (
        [
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5], // front
            [-0.5, 0.5, -0.5],
            [0.5, 0.5, -0.5],
            [0.5, -0.5, -0.5],
            [-0.5, -0.5, -0.5], // back
            [0.5, -0.5, 0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [0.5, 0.5, 0.5], // right
            [-0.5, -0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [-0.5, 0.5, 0.5],
            [-0.5, 0.5, -0.5], // left
            [-0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5], // top
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, -0.5, 0.5],
            [-0.5, -0.5, 0.5], // bottom
        ],
        [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, -1.0, 0.0],
        ],
    );

    let mut verts = Vec::with_capacity(24);
    // Per-corner UVs (planar mapping per face).
    let uvs: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (face, &color) in colors.iter().enumerate() {
        let tangent = face_tangent(normals[face * 4]);
        for (corner, &uv) in uvs.iter().enumerate() {
            let idx = face * 4 + corner;
            verts.push(Vertex {
                position: positions[idx],
                normal: normals[idx],
                color,
                uv,
                tangent,
            });
        }
    }
    verts
}

/// Pick a stable tangent for a face given its normal (used for the PBR debug
/// `Normal` (Tangent) view). Not strictly orthonormalized against the normal
/// but sufficient for visualization.
fn face_tangent(n: [f32; 3]) -> [f32; 3] {
    let mut up = [0.0f32, 1.0, 0.0];
    if (n[0] * n[0] + n[1] * n[1]).sqrt() < 1e-4 {
        up = [1.0, 0.0, 0.0];
    }
    let t = [
        up[1] * n[2] - up[2] * n[1],
        up[2] * n[0] - up[0] * n[2],
        up[0] * n[1] - up[1] * n[0],
    ];
    let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    if len < 1e-6 {
        [1.0, 0.0, 0.0]
    } else {
        [t[0] / len, t[1] / len, t[2] / len]
    }
}

fn cube_indices() -> Vec<u32> {
    let mut indices = Vec::with_capacity(36);
    for face in 0..6 {
        let base = (face * 4) as u32;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
    indices
}

// ---------------------------------------------------------------------------
// Sphere geometry (UV sphere)
// ---------------------------------------------------------------------------

fn sphere_mesh(sectors: u32, stacks: u32) -> (Vec<Vertex>, Vec<u32>) {
    let r = 0.5;
    let mut verts = Vec::new();
    let mut indices = Vec::new();

    for i in 0..=stacks {
        let phi = i as f32 * std::f32::consts::PI / stacks as f32;
        let (sp, cp) = phi.sin_cos();
        for j in 0..=sectors {
            let theta = j as f32 * 2.0 * std::f32::consts::PI / sectors as f32;
            let (st, ct) = theta.sin_cos();
            let pos = [r * sp * ct, r * cp, r * sp * st];
            // Analytic tangent along increasing theta (u direction).
            let mut tangent = [-sp * st, 0.0, sp * ct];
            let tlen = (tangent[0] * tangent[0] + tangent[2] * tangent[2]).sqrt();
            if tlen > 1e-6 {
                tangent = [tangent[0] / tlen, 0.0, tangent[2] / tlen];
            }
            let u = theta / (2.0 * std::f32::consts::PI);
            let v = phi / std::f32::consts::PI;
            verts.push(Vertex {
                position: pos,
                normal: [pos[0] / r, pos[1] / r, pos[2] / r],
                color: [0.85, 0.85, 0.85], // light gray
                uv: [u, v],
                tangent,
            });
        }
    }

    for i in 0..stacks {
        for j in 0..sectors {
            let first = i * (sectors + 1) + j;
            let second = first + sectors + 1;
            indices.extend_from_slice(&[first, first + 1, second]);
            indices.extend_from_slice(&[first + 1, second + 1, second]);
        }
    }

    (verts, indices)
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<World>,
    mesh_manager: MeshManager,
    input_state: InputState,
    camera: OrbitCamera,
    camera_controller: OrbitCameraController,
    needs_resize: bool,
    start: Instant,
    /// Optional equirectangular HDR environment map bytes (`.hdr`), threaded
    /// from the platform entry point into the renderer for image-based lighting.
    env_bytes: Option<Vec<u8>>,
    /// Currently selected PBR debug visualization mode.
    debug_mode: DebugMode,
    /// Coordinate space for the `Normal` debug mode.
    normal_space: NormalSpace,
    /// Whether the debug overlay UI is shown.
    show_ui: bool,
    /// P0: CPU-side scene storage (meshes / materials / textures / instances)
    /// populated either from a glTF file or from the procedural fallback.
    /// The renderer's `Render*Manager`s consume this on `App::load_demo_scene`.
    scene_store: prism_asset::SceneStore,
    /// Set to `true` once `App::load_demo_scene` has run; subsequent
    /// `resumed` callbacks reuse the registered resources instead of
    /// re-creating them.
    demo_objects_loaded: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            world: None,
            mesh_manager: MeshManager::new(),
            input_state: InputState::new(),
            camera: OrbitCamera::new(16.0 / 9.0),
            camera_controller: OrbitCameraController::default(),
            needs_resize: false,
            start: Instant::now(),
            env_bytes: None,
            debug_mode: DebugMode::Final,
            normal_space: NormalSpace::World,
            show_ui: true,
            scene_store: prism_asset::SceneStore::new(),
            demo_objects_loaded: false,
        }
    }

    /// Create and run the application on a new event loop (desktop). Loads the
    /// environment map from `assets/env.hdr` (if present) for IBL.
    pub fn run() -> anyhow::Result<()> {
        let env_bytes = load_env_bytes();
        Self::run_on_event_loop_with_env(EventLoop::new()?, env_bytes)
    }

    /// Run the application on an existing event loop (used by Android).
    pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
        Self::run_on_event_loop_with_env(event_loop, None)
    }

    /// Run on an existing event loop with an explicit environment map payload.
    pub fn run_on_event_loop_with_env(
        event_loop: EventLoop<()>,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        Self::run_on_event_loop_with_env_and_scene(event_loop, env_bytes, None)
    }

    /// Variant that also threads an in-memory glTF scene (the bytes
    /// of a `.glb` or `.gltf` file) into the engine. Used by the
    /// Android entry point, which reads the asset via `AssetManager::open`
    /// before `winit` takes over. The scene is loaded into the
    /// `SceneStore` immediately; `load_demo_scene` then uploads the
    /// contents to the renderer on the first `resumed` callback.
    pub fn run_on_event_loop_with_env_and_scene(
        event_loop: EventLoop<()>,
        env_bytes: Option<Vec<u8>>,
        scene_glb: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        let mut app = App::new();
        app.env_bytes = env_bytes;
        if let Some(bytes) = scene_glb {
            match app.scene_store.load_gltf_bytes(&bytes, None) {
                Ok(h) => log::info!("App: preloaded glTF scene {:?}", h),
                Err(e) => log::warn!("App: failed to preload glTF scene: {e}"),
            }
        }
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("PrismaRev")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600, 900)),
                )
                .expect("failed to create window"),
        );

        // Instance extensions from the surface.
        let display_handle = window.display_handle().expect("get display handle").into();
        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
            .expect("enumerate required extensions");
        let extensions: Vec<String> = ext_ptrs
            .iter()
            .map(|p| {
                unsafe { std::ffi::CStr::from_ptr(*p) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();

        let renderer = Renderer::new(
            extensions_ref,
            window.as_ref(),
            window.as_ref(),
            self.env_bytes.clone(),
        )
        .expect("failed to create renderer");

        // --- Build test scene: sphere + cubes ---
        let (world, mesh_manager) = create_test_scene(&renderer);

        self.world = Some(world);
        self.mesh_manager = mesh_manager;
        self.window = Some(window);
        self.renderer = Some(renderer);

        // Update camera aspect ratio to match initial window size.
        self.camera = OrbitCamera::new(1600.0 / 900.0);
    }

    // ---- P0: scene loading (commit 10) ---------------------------------
    //
    // `load_demo_scene` is the entry point that turns a `SceneStore`
    // (either populated from a glTF file or from the procedural
    // fallback) into a set of registered mesh / material / texture
    // resources on the renderer's managers. It is idempotent — calling
    // it twice is a no-op — so the winit `resumed` callback can invoke
    // it on every (re)start without re-registering everything.
    //
    // The draw path that actually consumes the new manager state is
    // wired up in a follow-up commit (commit 11) once the
    // bindless-frag pipeline + descriptor-set layout is in place. P0
    // is about getting the manager lifecycle + scene loading path
    // covered.
    pub fn load_demo_scene(&mut self, scene: prism_asset::SceneHandle) {
        if self.demo_objects_loaded {
            log::debug!("App::load_demo_scene: already loaded, skipping");
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            log::debug!("App::load_demo_scene: no renderer yet, deferring");
            return;
        };

        // 1. Register every texture into the bindless SRV table.
        let texture_data: Vec<_> = self
            .scene_store
            .textures()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        for (asset_h, data) in texture_data {
            let input = prism_render::managers::TextureUploadInput {
                width: data.width,
                height: data.height,
                format: match data.format {
                    prism_asset::TexFormat::Rgba8 => {
                        prism_render::managers::TextureFormat::Rgba8
                    }
                    prism_asset::TexFormat::Rgba16f => {
                        log::warn!("Rgba16f texture not yet supported by render manager; skipping");
                        continue;
                    }
                },
                pixels: data.pixels.clone(),
            };
            // P0: only the slot is reserved; the renderer will wire
            // the actual vk::ImageView in commit 11. The handle is
            // not used until then, but the reservation is enough to
            // assert capacity and surface pixel-buffer errors early.
            if let Err(e) = renderer.register_texture(&input) {
                log::warn!("register_texture failed: {e}");
            }
            // Keep the asset handle alive to avoid a stale-borrow
            // warning; the asset handle is the input the material
            // step below uses to resolve texture references.
            let _ = asset_h;
        }

        // 2. Register every material with placeholder bindless slots
        // (u32::MAX = "no texture"). Commit 11 fills the slots in
        // once texture views are created.
        let material_data: Vec<_> = self
            .scene_store
            .materials()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        for (asset_h, data) in material_data {
            let input = prism_render::managers::MaterialUploadInput {
                base_color: data.base_color,
                metallic: data.metallic,
                roughness: data.roughness,
                emissive: data.emissive,
                albedo_tex: data
                    .albedo_tex
                    .and_then(|h| self.scene_store.texture(h).map(|_| u32::MAX)),
                normal_tex: data
                    .normal_tex
                    .and_then(|h| self.scene_store.texture(h).map(|_| u32::MAX)),
                metallic_roughness_tex: data
                    .metallic_roughness_tex
                    .and_then(|h| self.scene_store.texture(h).map(|_| u32::MAX)),
                emissive_tex: data
                    .emissive_tex
                    .and_then(|h| self.scene_store.texture(h).map(|_| u32::MAX)),
            };
            if let Err(e) = renderer.register_material(input) {
                log::warn!("register_material failed: {e}");
            }
            let _ = asset_h;
        }

        // 3. Register every mesh (uploads vertex + index buffers).
        let mesh_data: Vec<_> = self
            .scene_store
            .meshes()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        for (asset_h, data) in mesh_data {
            let input = prism_render::managers::MeshUploadInput {
                positions: data.positions.clone(),
                normals: data.normals.clone(),
                colors: vec![],
                uvs: data.uvs.clone(),
                tangents: data.tangents.clone(),
                indices: data.indices.clone(),
            };
            if let Err(e) = renderer.register_mesh(&input) {
                log::warn!("register_mesh failed: {e}");
            }
            let _ = asset_h;
        }

        // 4. Flush any pending material edits to the GPU.
        if let Err(e) = renderer.flush_materials() {
            log::warn!("flush_materials failed: {e}");
        }

        // The unused `scene` parameter is kept for future use (e.g. to
        // bind the scene to specific instance groups).
        let _ = scene;
        self.demo_objects_loaded = true;
        log::info!(
            "App::load_demo_scene: registered {} mesh(es), {} material(s), {} texture(s)",
            self.scene_store.meshes().count(),
            self.scene_store.materials().count(),
            self.scene_store.textures().count(),
        );
    }

    /// Convenience: try to load a glTF scene from disk. On success the
    /// scene is appended to the `SceneStore`; the caller is expected to
    /// follow up with `load_demo_scene` to upload the contents to the
    /// renderer. Returns `None` when the file is missing or the parse
    /// fails — both are non-fatal so the demo can fall back to a
    /// procedural scene.
    pub fn try_load_gltf(&mut self, path: &std::path::Path) -> Option<prism_asset::SceneHandle> {
        match self.scene_store.load_gltf(path) {
            Ok(h) => {
                log::info!("App::try_load_gltf: loaded {}", path.display());
                Some(h)
            }
            Err(e) => {
                log::warn!("App::try_load_gltf: {} (continuing with procedural fallback)", e);
                None
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // First start (or after full teardown): build everything.
            self.ensure_window(event_loop);
            return;
        }
        // Window already exists → this is a resume after suspend (e.g. Android
        // screen lock/unlock). The OS invalidated the VkSurfaceKHR while we
        // were suspended; rebuild only the surface-dependent resources,
        // reusing the VulkanContext, render pass, pipeline, descriptors, UBOs,
        // command pool, and shaders.
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if renderer.has_swapchain() {
            // Already live (e.g. desktop spurious resume); nothing to do.
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        match renderer.resume_surface(window.as_ref(), window.as_ref()) {
            Ok(()) => {
                log::info!("resume_surface ok; resuming rendering");
                self.needs_resize = false; // resume already sized correctly
            }
            Err(e) => {
                // Don't crash — rendering stays suspended; next resize/redraw
                // will retry. Common during transitions.
                log::warn!("resume_surface failed (will retry): {e}");
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The window's surface is about to become invalid (Android onPause /
        // screen lock). Drop surface-dependent resources now so we don't
        // touch a dead VkSurfaceKHR on the next frame. Device-bound resources
        // are retained by the renderer for fast resume.
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.suspend_surface();
        }
        // NOTE: keep self.window — on Android the winit window handle remains
        // valid across suspend; only the underlying surface needs rebuilding.
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                log::info!(
                    "Resized: {}x{} aspect={:.4}",
                    size.width,
                    size.height,
                    if size.height > 0 {
                        size.width as f32 / size.height as f32
                    } else {
                        0.0
                    },
                );
                if size.width > 0 && size.height > 0 {
                    let aspect = size.width as f32 / size.height as f32;
                    self.camera.set_aspect(aspect);
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_one_frame();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Left-click: try the debug overlay first; if it consumes the
                // click, don't also start a camera drag.
                if state == winit::event::ElementState::Pressed
                    && button == winit::event::MouseButton::Left
                {
                    let pos = self.input_state.mouse_position();
                    let ext = self.renderer.as_ref().map(|r| r.extent());
                    log::info!(
                        "MOUSE_DEBUG pos=({:.1},{:.1}) extent={:?}",
                        pos[0],
                        pos[1],
                        ext.map(|e| (e.width, e.height)),
                    );
                    if self.handle_overlay_click(pos[0] as f32, pos[1] as f32) {
                        return;
                    }
                }
                self.input_state.handle_mouse_button(button.into(), state);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input_state.handle_mouse_move([position.x, position.y]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input_state.handle_scroll(delta);
            }
            WindowEvent::Touch(touch) => {
                // Map single-touch drag to a left mouse drag so the existing
                // orbit controller works unchanged on touch devices.
                let pos = [touch.location.x, touch.location.y];
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        self.input_state.set_mouse_position(pos);
                        let ext = self.renderer.as_ref().map(|r| r.extent());
                        let orient = self.renderer.as_ref().map(|r| r.orientation());
                        log::info!(
                            "TOUCH_DEBUG touch.location=({:.1},{:.1}) extent={:?} \
                             orientation_aspect={:.4} rotation={:?}",
                            pos[0],
                            pos[1],
                            ext.map(|e| (e.width, e.height)),
                            orient.map(|o| o.0).unwrap_or(1.0),
                            orient.map(|o| o.1),
                        );
                        if self.handle_overlay_click(pos[0] as f32, pos[1] as f32) {
                            // Consumed by the overlay; don't start a camera drag.
                        } else {
                            self.input_state.handle_mouse_button(
                                MouseButton::Left,
                                winit::event::ElementState::Pressed,
                            );
                        }
                    }
                    winit::event::TouchPhase::Moved => {
                        self.input_state.handle_mouse_move(pos);
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        self.input_state.handle_mouse_button(
                            MouseButton::Left,
                            winit::event::ElementState::Released,
                        );
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key,
                        state,
                        ..
                    },
                ..
            } => {
                if state == winit::event::ElementState::Pressed {
                    if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                        match code {
                            KeyCode::Digit1 => self.debug_mode = DebugMode::Final,
                            KeyCode::Digit2 => self.debug_mode = DebugMode::Albedo,
                            KeyCode::Digit3 => self.debug_mode = DebugMode::Specular,
                            KeyCode::Digit4 => self.debug_mode = DebugMode::Reflection,
                            KeyCode::Digit5 => self.debug_mode = DebugMode::Ambient,
                            KeyCode::Digit6 => self.debug_mode = DebugMode::Normal,
                            KeyCode::KeyN => self.normal_space = self.normal_space.next(),
                            KeyCode::KeyH => self.show_ui = !self.show_ui,
                            _ => {}
                        }
                    }
                }
                self.input_state.handle_keyboard(physical_key, state);
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            // Absolute mouse motion from raw device events.
            // On some platforms (Linux/Windows raw input) CursorMoved may not
            // fire reliably while a button is held; MouseMotion supplements it.
            let pos = self.input_state.mouse_position();
            self.input_state
                .handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // Wait for the GPU to finish any in-flight work (e.g. the last
            // frame's command buffer) before destroying mesh buffers. Without
            // this, vkDestroyBuffer is called on buffers still referenced by a
            // submitted command buffer (VUID-vkDestroyBuffer-buffer-00922).
            if let Some(renderer) = self.renderer.as_ref() {
                unsafe { renderer.context().device.device_wait_idle().ok() };
            }
            for mut mesh in std::mem::take(&mut self.mesh_manager).into_meshes() {
                if let Some(ref renderer) = self.renderer {
                    unsafe { mesh.destroy(&renderer.context().device) };
                }
            }
            self.renderer = None;
            self.world = None;
            self.window = None;
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    /// Hit-test a pointer against the debug overlay and apply the resulting
    /// action. Returns `true` if the overlay consumed the click (so the caller
    /// should not also treat it as a camera drag).
    fn handle_overlay_click(&mut self, px: f32, py: f32) -> bool {
        if !self.show_ui {
            return false;
        }
        let action = self
            .renderer
            .as_ref()
            .and_then(|r| r.hit_test_overlay(px, py));
        match action {
            Some(OverlayAction::SetMode(m)) => {
                self.debug_mode = m;
                true
            }
            Some(OverlayAction::CycleNormalSpace) => {
                self.normal_space = self.normal_space.next();
                true
            }
            None => false,
        }
    }

    fn render_one_frame(&mut self) {
        // Skip rendering while the surface is suspended (no swapchain).
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if !renderer.has_swapchain() {
            return;
        }

        // Handle pending resize.
        if self.needs_resize {
            self.needs_resize = false;
            if let Some(renderer) = self.renderer.as_mut() {
                if let Err(e) = renderer.recreate_swapchain() {
                    log::debug!("swapchain recreate deferred: {e}");
                    return;
                }
            }
        }

        // Update camera from input state (events populated by window_event).
        self.camera_controller
            .update(&mut self.camera, &self.input_state);
        // Clear transient input state for the next frame.
        self.input_state.begin_frame();

        // Animate cubes: rotate around Y axis.
        let elapsed = self.start.elapsed().as_secs_f32();
        if let Some(world) = self.world.as_mut() {
            for (_, transform) in world.query_mut::<Transform>() {
                let angle = elapsed * 0.5; // 0.5 rad/s ≈ 29°/s
                let half = angle * 0.5;
                transform.rotation = [0.0, half.sin(), 0.0, half.cos()];
            }
        }

        // Build light data (directional).
        // Light: 45° diagonal in XY plane (upper-left), Z=0.
        let light_dir = [-1.0f32, 1.0, 0.0];
        let light_dir_len = (light_dir[0] * light_dir[0]
            + light_dir[1] * light_dir[1]
            + light_dir[2] * light_dir[2])
            .sqrt();
        let light_direction = [
            light_dir[0] / light_dir_len,
            light_dir[1] / light_dir_len,
            light_dir[2] / light_dir_len,
            1.0, // intensity
        ];
        let light_color = [1.0, 1.0, 1.0, 0.1]; // white, ambient factor 0.1

        let light_data = FrameUBOData {
            view_proj: [[0.0; 4]; 4],  // placeholder, render_system fills it
            camera_position: [0.0; 4], // placeholder
            light_direction,
            light_color,
            view: [[0.0; 4]; 4], // placeholder, render_system fills it
        };

        let clear_color = [0.05, 0.05, 0.1, 1.0]; // dark blue-gray

        let (renderer, world, meshes) = match (
            self.renderer.as_mut(),
            self.world.as_ref(),
            &self.mesh_manager,
        ) {
            (Some(r), Some(w), m) => (r, w, m),
            _ => return,
        };

        render_system(
            renderer,
            world,
            meshes,
            clear_color,
            &mut self.camera,
            &light_data,
            self.debug_mode as u32,
            self.normal_space as u32,
            self.show_ui,
        );
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
