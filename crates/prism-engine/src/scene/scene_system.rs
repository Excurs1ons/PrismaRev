//! 场景系统——负责场景加载、切换与「当前场景」状态。
//!
//! # 职责
//!
//! [`SceneManager`] 是引擎的场景子系统：它拥有「当前场景名」，并统一负责
//! 从 `assets/scenes.toml` manifest 加载场景到 ECS 世界。场景**内容**只按
//! [`prism_asset::core::AssetId`] 从资源包（`.pak`）加载：`scenes.toml` 的
//! `path` 只是路径清单（`.pak.meta.json`）中的查找键，先解析为 `AssetId`，
//! 再由 [`ResourceManager`] 读取 RSCN 字节——不做任何磁盘文件回退。
//! 早期版本由 `Engine` 直接扫描 manifest 加载场景，现收拢到本系统
//! （`Engine::init_scene` 只做转发）——这是 todo「场景加载应该由场景系统
//! 负责」的落地。
//!
//! # 存档
//!
//! 存档（存/读档）**不是**引擎/场景系统的职责——`Engine` 不再在初始化时
//! 自动读取 `scene_state.json`，只通过 `Engine::save_scene_state` /
//! `Engine::load_scene_state` 暴露 [`crate::scene_state`] 原语，由用户项目
//! 决定何时调用。

use std::path::PathBuf;

use prism_asset::core::AssetId;
use prism_asset::runtime::{ResourceManager, SceneAsset};
use prism_ecs::World;

use super::component_registry::ComponentRegistry;
use super::loader::{SceneInstance, SceneLoader, SceneSource};

/// 场景系统：拥有当前场景状态并负责场景加载。
pub struct SceneManager {
    /// 当前已加载场景名（manifest 中 `scenes[].name`）。
    current_scene_name: Option<String>,
    environment_bytes: Option<Vec<u8>>,
}

/// 当前场景环境资源的中性读取接口。
pub trait EnvironmentProvider {
    fn load_environment(&mut self, asset_path: &str) -> Option<Vec<u8>>;
}

/// 渲染/资源阶段使用的只读 ECS 场景视图。
pub struct SceneReadView<'a> {
    world: &'a World,
}

impl<'a> SceneReadView<'a> {
    pub fn new(world: &'a World) -> Self {
        Self { world }
    }

    pub fn environment_asset_path(&self) -> Option<String> {
        self.world
            .query::<super::components::EnvironmentLighting>()
            .next()
            .and_then(|(_, env)| env.env_map.clone())
    }

    /// 提取渲染所需的扁平绘制列表，不暴露可变 ECS 访问。
    pub fn draw_items(&self) -> Vec<prism_render::DrawItem> {
        self.world
            .query3::<
                super::components::WorldTransform,
                super::components::MeshRef,
                super::components::MaterialRef,
            >()
            .filter(|(e, _, _, _)| {
                self.world
                    .get::<super::components::Active>(*e)
                    .map(|a| a.0)
                    .unwrap_or(true)
            })
            .map(|(_, wt, mr, mat)| prism_render::DrawItem {
                mesh: mr.render_handle,
                model: wt.0.to_cols_array_2d(),
                material: Some(mat.material_slot),
            })
            .collect()
    }
}

impl Default for SceneManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneManager {
    /// 创建一个空的场景系统（尚无当前场景）。
    pub fn new() -> Self {
        Self {
            current_scene_name: None,
            environment_bytes: None,
        }
    }

    /// 当前场景名；尚未从 manifest 加载任何场景时为 `None`。
    pub fn current_scene_name(&self) -> Option<&str> {
        self.current_scene_name.as_deref()
    }

    /// 注入当前场景的环境贴图字节。字节由运行时资源服务提供，场景系统
    /// 不负责文件读取或格式解码。
    pub fn set_environment_bytes(&mut self, bytes: Option<Vec<u8>>) {
        self.environment_bytes = bytes;
    }

    /// 从 `assets/scenes.toml` manifest 加载**第一个**已注册进资源包的场景到世界。
    ///
    /// 引擎初始化（`Engine::init_scene`）会委托本方法：场景一律按 `AssetId`
    /// 从 [`ResourceManager`] 加载（`scenes.toml` 的 `path` → 路径清单 → `AssetId`）。
    /// 返回加载成功的场景名并记为当前场景；无 manifest / 无可用场景时返回 `None`。
    pub fn load_first_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
    ) -> Option<String> {
        self.load_from_manifest(rm, world, registry, None)
    }

    /// 从 manifest 加载**指定名字**的场景到世界（场景切换入口）。
    ///
    /// 清单中找不到该名字或加载失败时，保持「当前场景」不变并返回 `None`。
    pub fn load_named_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        name: &str,
    ) -> Option<String> {
        self.load_from_manifest(rm, world, registry, Some(name))
    }

    /// 从调用方提供的内存 manifest 加载场景。
    ///
    /// 运行时宿主可以从 `.pak`、Android asset 或网络资源注入清单，避免
    /// 场景系统依赖当前工作目录。文件入口仅作为兼容层保留。
    pub fn load_from_manifest_text(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        text: &str,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let manifest: SceneManifest = match toml::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("scene manifest parse error: {e}");
                return None;
            }
        };
        self.load_entries(rm, world, registry, &manifest, scene_name)
    }

    /// 解析「当前场景」声明式光照所需的环境贴图字节。
    ///
    /// 取代旧的 [`load_env_bytes_from_manifest`]，后者扫描 manifest 所有 `.rscn`，
    /// 导致 UI 开屏也加载 3D 环境贴图。新逻辑按 `current_scene_name` 定位场景：
    /// - `.rscn`：复用 `read_env_path_from_rscn` 自动派生（向后兼容，default/sponza
    ///   的 IBL 无需改场景即保留）。
    /// - `.scene.json`：从已加载的 ECS 世界查询 `EnvironmentLighting` 组件；
    ///   无该组件或 `env_map = None` → 返回 `None`（开屏等无光照场景不构建 IBL）。
    #[cfg(feature = "legacy-disk-scenes")]
    #[deprecated(note = "use current_scene_env_bytes_with_provider; this fallback reads scene files")]
    pub fn current_scene_env_bytes(&self, world: &World) -> Option<Vec<u8>> {
        if let Some(bytes) = &self.environment_bytes {
            return Some(bytes.clone());
        }
        let name = self.current_scene_name.as_ref()?;
        let (manifest_dir, manifest) = find_and_parse_manifest()?;
        let entry = manifest.scenes.iter().find(|e| &e.name == name)?;
        let path = manifest_dir.join(&entry.path);

        if entry.path.ends_with(".rscn") {
            if let Some(hdr_rel) = super::loader::read_env_path_from_rscn(&path) {
                let hdr_path = path
                    .parent()
                    .map(|d| d.join(&hdr_rel))
                    .unwrap_or_else(|| PathBuf::from(&hdr_rel));
                match std::fs::read(&hdr_path) {
                    Ok(bytes) => {
                        log::info!("scene '{}' environment (rscn): {}", name, hdr_path.display());
                        return Some(bytes);
                    }
                    Err(e) => log::warn!("env map {} not readable: {e}", hdr_path.display()),
                }
            }
            return None;
        }

        if entry.path.ends_with(".scene.json") {
            for (_, env) in world.query::<super::components::EnvironmentLighting>() {
                if let Some(env_map) = &env.env_map {
                    let hdr_path = path
                        .parent()
                        .map(|d| d.join(env_map))
                        .unwrap_or_else(|| PathBuf::from(env_map));
                    match std::fs::read(&hdr_path) {
                        Ok(bytes) => {
                            log::info!(
                                "scene '{}' environment (component): {}",
                                name,
                                hdr_path.display()
                            );
                            return Some(bytes);
                        }
                        Err(e) => {
                            log::warn!("env map {} not readable: {e}", hdr_path.display())
                            // 组件声明了 env_map 但文件缺失：视为无 IBL。
                        }
                    }
                }
                // 找到 EnvironmentLighting 组件但无 env_map（或文件缺失）→ 不构建 IBL。
                return None;
            }
        }
        None
    }

    /// 使用调用方提供的资源读取器解析环境贴图，不访问文件系统。
    pub fn current_scene_env_bytes_with_provider<P: EnvironmentProvider>(
        &self,
        view: SceneReadView<'_>,
        provider: &mut P,
    ) -> Option<Vec<u8>> {
        if let Some(bytes) = &self.environment_bytes {
            return Some(bytes.clone());
        }
        if let Some(path) = view.environment_asset_path() {
            return provider.load_environment(&path);
        }
        None
    }

    // -----------------------------------------------------------------------
    // 内部：manifest 驱动加载
    // -----------------------------------------------------------------------

    /// 按 manifest 逐个解析场景并加载；`scene_name = None` 表示首个可加载场景。
    fn load_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let manifest_path = find_manifest_path();
        let manifest_path = match manifest_path {
            Some(p) => p,
            None => {
                let cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "<unknown>".into());
                log::info!("no assets/scenes.toml found (cwd={cwd}); using procedural demo only");
                return None;
            }
        };
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("failed to read scene manifest {:?}: {e}", manifest_path);
                return None;
            }
        };
        log::info!("scene manifest: {:?} ({} bytes)", manifest_path, text.len());
        let manifest: SceneManifest = match toml::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("scene manifest parse error: {e}");
                return None;
            }
        };
        log::info!(
            "scene manifest parsed: {} scene(s) listed",
            manifest.scenes.len()
        );
        self.load_entries(rm, world, registry, &manifest, scene_name)
    }

    fn load_entries(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        manifest: &SceneManifest,
        scene_name: Option<&str>,
    ) -> Option<String> {
        for entry in &manifest.scenes {
            if let Some(name) = scene_name {
                if &entry.name != name {
                    continue;
                }
            }
            // 场景内容一律按 AssetId 从资源包加载：scenes.toml 的 `path` 只是
            // 路径清单（.pak.meta.json）中的查找键，先解析为 AssetId，再由
            // ResourceManager 读取 RSCN 字节——不做磁盘文件回退。
            let Some(id) = rm.id_by_path(&entry.path) else {
                log::warn!(
                    "scene '{}' -> '{}' not registered in resource package; skipping",
                    entry.name,
                    entry.path
                );
                continue;
            };
            match load_scene_by_id(rm, world, id, registry) {
                Ok(inst) => {
                    log::info!(
                        "scene '{}' loaded: {} entities ({} roots)",
                        entry.name,
                        inst.all_entities.len(),
                        inst.root_entities.len()
                    );
                    self.current_scene_name = Some(entry.name.clone());
                    return Some(entry.name.clone());
                }
                Err(e) => {
                    log::warn!("scene '{}' failed to load: {e}", entry.name);
                    continue;
                }
            }
        }
        log::info!("no resolvable scene in manifest; using procedural demo only");
        None
    }
}

/// 加载 environment 映射表字节 from the **第一个** scene in `scenes.toml`.
///
/// Legacy 手动入口——已被 [`SceneManager::current_scene_env_bytes`] 取代
/// （后者只解析**当前**场景，避免 UI 开屏也加载 3D 环境贴图）。
#[cfg(feature = "legacy-disk-scenes")]
#[deprecated(note = "load environment data through EnvironmentProvider")]
pub fn load_env_bytes_from_manifest() -> Option<Vec<u8>> {
    let (manifest_dir, manifest) = find_and_parse_manifest()?;
    for entry in &manifest.scenes {
        let path = manifest_dir.join(&entry.path);
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false)
            || !path.exists()
        {
            continue;
        }
        if let Some(hdr_rel) = super::loader::read_env_path_from_rscn(&path) {
            let hdr_path = path
                .parent()
                .map(|d| d.join(&hdr_rel))
                .unwrap_or_else(|| PathBuf::from(&hdr_rel));
            match std::fs::read(&hdr_path) {
                Ok(bytes) => {
                    log::info!("loaded environment map from scene: {}", hdr_path.display());
                    return Some(bytes);
                }
                Err(e) => log::warn!("env map HDR {} not readable: {e}", hdr_path.display()),
            }
        }
    }
    log::info!("no environment map in scene manifest; using procedural fallback");
    None
}

// ---------------------------------------------------------------------------
// Manifest helpers（由旧 engine.rs 迁入）——场景系统的内部实现
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct SceneManifestEntry {
    pub name: String,
    pub path: String,
}

#[derive(serde::Deserialize)]
pub struct SceneManifest {
    pub scenes: Vec<SceneManifestEntry>,
}

/// 不依赖当前工作目录地定位 `scenes.toml`：
/// 依次在【当前目录及其各级父目录】与【可执行文件所在目录及其各级父目录】下，
/// 尝试 `assets/scenes.toml` 与 `crates/prism-engine/assets/scenes.toml`。
///
/// 这样无论从仓库根还是 `game/` 子目录（如 `cd game && cargo run`）启动，
/// 都能正确找到资源清单——之前 `cargo run` 把 cwd 设为 `game/`，导致
/// `assets/scenes.toml` 找不到、回退到空场景而黑屏。
fn find_manifest_path() -> Option<PathBuf> {
    let rels = ["assets/scenes.toml", "crates/prism-engine/assets/scenes.toml"];
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        bases.push(cwd.clone());
        let mut p = cwd.as_path();
        while let Some(parent) = p.parent() {
            bases.push(parent.to_path_buf());
            p = parent;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            bases.push(exe_dir.to_path_buf());
            let mut p = exe_dir;
            while let Some(parent) = p.parent() {
                bases.push(parent.to_path_buf());
                p = parent;
            }
        }
    }
    for base in bases {
        for rel in rels {
            let candidate = base.join(rel);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(feature = "legacy-disk-scenes")]
fn find_and_parse_manifest() -> Option<(PathBuf, SceneManifest)> {
    let manifest_path = find_manifest_path()?;
    let manifest_dir = manifest_path.parent()?.to_path_buf();
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: SceneManifest = toml::from_str(&text).ok()?;
    log::info!(
        "scene manifest: {:?} ({} entries)",
        manifest_path,
        manifest.scenes.len()
    );
    Some((manifest_dir, manifest))
}

fn load_scene_by_id(
    rm: &mut ResourceManager,
    world: &mut World,
    id: AssetId,
    registry: &ComponentRegistry,
) -> Result<SceneInstance, anyhow::Error> {
    use anyhow::Context;
    let handle = rm
        .load_with_deps::<SceneAsset>(id)
        .with_context(|| format!("load scene {id}"))?;
    let asset = rm
        .get::<SceneAsset>(handle)
        .with_context(|| format!("get scene {id}"))?;
    let mut loader = SceneLoader::new();
    loader
        .load_and_spawn(world, SceneSource::RawCooked(asset.bytes.clone()), registry)
        .map_err(|e| anyhow::anyhow!("{e}"))
}
