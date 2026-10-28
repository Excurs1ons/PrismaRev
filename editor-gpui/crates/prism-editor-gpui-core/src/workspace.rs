//! 编辑器工作区状态：打开的文档、选择、dock 布局、主题、日志、Play 状态。
//!
//! 这一层**不依赖任何 UI 框架**，GPUI 只是它的一个视图。因此同一份状态可以
//! 单测，也可以在将来喂给别的宿主（例如自动化脚本）。

use crate::assets::{AssetInfo, AssetKind, AssetQuery, AssetTree};
use crate::doc::{EntityJson, SceneDoc, TransformJson, PREFAB_LINK_COMPONENT};
use crate::dock::{
    self, collect_tabs, find_leaf, find_leaf_by_id, insert_panel, leaf, remove_panel, DockEdge,
    DockNode, DockZone, FloatPanel, LAYOUT_PRESETS,
};
use crate::i18n::{I18n, Locale};
use crate::keys::Command;
use crate::logbuf::{LogBuffer, LogLevel, LogSource};
use crate::manifest::SceneManifest;
use crate::project::{self, ProjectFile};
use crate::schema::{component_value_with_overrides, EntityPreset, Schema, TypeSchema};
use crate::theme::{ThemeColors, ThemeState, ThemeToken};
use crate::viewport_math::{
    basis_from_quat, parent_world_rotation, quat_axis_angle, quat_conjugate, quat_mul,
    quat_normalize, quat_rotate, world_rotation, GizmoSpace,
};
use serde_json::Value;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    Scene,
    Prefab,
}

impl DocKind {
    pub fn folder(self) -> &'static str {
        match self {
            DocKind::Scene => "assets/scenes",
            DocKind::Prefab => "assets/prefabs",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            DocKind::Scene => ".scene.json",
            DocKind::Prefab => ".prefab.json",
        }
    }

    pub fn i18n_key(self) -> &'static str {
        match self {
            DocKind::Scene => "common.scene",
            DocKind::Prefab => "common.prefab",
        }
    }

    pub fn parse(text: &str) -> DocKind {
        if text == "prefab" {
            DocKind::Prefab
        } else {
            DocKind::Scene
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    pub kind: DocKind,
    pub path: Option<String>,
    pub doc: SceneDoc,
    pub undo: crate::undo::UndoStack,
    pub dirty: bool,
}

impl Document {
    pub fn new(kind: DocKind, path: Option<String>, doc: SceneDoc) -> Self {
        Self {
            kind,
            path,
            doc,
            undo: crate::undo::UndoStack::new(),
            dirty: false,
        }
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(path) => path.rsplit('/').next().unwrap_or(path).to_string(),
            None => match self.kind {
                DocKind::Scene => "Untitled.scene.json".to_string(),
                DocKind::Prefab => "Untitled.prefab.json".to_string(),
            },
        }
    }

    pub fn title_with_dirty(&self) -> String {
        if self.dirty {
            format!("{} *", self.title())
        } else {
            self.title()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PanelInfo {
    pub id: String,
    pub title: String,
    pub menu_path: String,
    pub shortcut: Option<String>,
    /// 辅助窗口（工具/设置/分析）默认并进检查器 leaf。
    pub utility: bool,
}

impl PanelInfo {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            menu_path: String::new(),
            shortcut: None,
            utility: dock::UTILITY_TABS.contains(&id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateDoc {
    pub id: String,
    pub label: String,
    pub kind: DocKind,
    pub doc: SceneDoc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentClipboard {
    pub component: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityClipboard {
    /// 每个顶层选中实体对应一棵独立子树（id 已从 1 重新编号）。
    pub trees: Vec<SceneDoc>,
    /// 是否来自剪切（仅用于界面提示，粘贴行为相同）。
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0, 0.0],
            yaw: 0.6,
            pitch: 0.35,
            distance: 12.0,
        }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        [
            self.target[0] + self.distance * cos_pitch * sin_yaw,
            self.target[1] + self.distance * sin_pitch,
            self.target[2] + self.distance * cos_pitch * cos_yaw,
        ]
    }

    /// 拖动轨道（像素 → 弧度）。
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.5, 1.5);
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (1.0 - delta * 0.001)).clamp(0.2, 500.0);
    }

    pub fn focus(&mut self, point: [f32; 3], radius: f32) {
        self.target = point;
        // 垂直 fov 60°：距离 = 半径 / sin(fov/2) 留 20% 余量。
        self.distance = (radius / (30.0f32.to_radians().sin()) * 1.2).max(0.5);
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub docs: Vec<Document>,
    active: usize,
    pub schema: Schema,
    pub presets: Vec<EntityPreset>,
    pub templates: Vec<TemplateDoc>,
    pub assets: Vec<AssetInfo>,
    pub tree: AssetTree,
    pub asset_query: AssetQuery,
    pub expanded_assets: HashSet<String>,
    pub panels: Vec<PanelInfo>,
    pub dock: DockNode,
    pub floating: Vec<FloatPanel>,
    pub selection: Vec<usize>,
    pub expanded: HashSet<u32>,
    pub clipboard: Option<ComponentClipboard>,
    /// 实体剪贴板（Ctrl+C/X/V）：每项是一棵已重新编号的子树。
    pub entity_clipboard: Option<EntityClipboard>,
    pub theme: ThemeState,
    pub i18n: I18n,
    pub log: LogBuffer,
    pub camera: OrbitCamera,
    /// W/E/R gizmo 使用世界轴还是实体自身轴。
    pub gizmo_space: GizmoSpace,
    /// Scene View 是否画世界网格。
    pub show_grid: bool,
    pub playing: bool,
    pub paused: bool,
    pub play_time_s: f32,
    pub status: String,
    pub layout_preset: String,
    /// 项目根（含 `assets/`）；`None` = 纯 fixture / 内置样例模式。
    pub project_root: Option<PathBuf>,
    /// 磁盘上的场景 / 预制体（`assets/scenes`、`assets/prefabs`）。
    pub project_files: Vec<ProjectFile>,
    /// `assets/scenes.toml` 的场景清单（顺序 = 构建 / 启动顺序）。
    pub scene_manifest: SceneManifest,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::sample()
    }
}

impl Workspace {
    /// 一个自包含的最小工作区（没有 fixture 时也能启动）。
    pub fn sample() -> Self {
        let mut doc = SceneDoc::default();
        let camera = doc.add_named_child(None, "Main Camera");
        doc.entities[camera].transform = TransformJson {
            position: [0.0, 1.0, -10.0],
            ..Default::default()
        };
        let cube = doc.add_named_child(None, "Cube");
        doc.entities[cube].transform.position = [0.0, 0.5, 0.0];
        let light = doc.add_named_child(None, "Directional Light");
        doc.entities[light].transform.rotation = [0.3, 0.2, 0.1, 0.9];
        let _ = doc.add_named_child(Some(cube), "Detail");

        let mut workspace = Self {
            docs: vec![Document::new(DocKind::Scene, None, doc)],
            active: 0,
            schema: Schema::default(),
            presets: Vec::new(),
            templates: Vec::new(),
            assets: Vec::new(),
            tree: AssetTree::default(),
            asset_query: AssetQuery::default(),
            expanded_assets: HashSet::new(),
            panels: default_panels(),
            dock: dock::default_dock(),
            floating: Vec::new(),
            selection: Vec::new(),
            expanded: HashSet::new(),
            clipboard: None,
            entity_clipboard: None,
            theme: ThemeState::default(),
            i18n: I18n::default(),
            log: LogBuffer::default(),
            camera: OrbitCamera::default(),
            gizmo_space: GizmoSpace::World,
            show_grid: true,
            playing: false,
            paused: false,
            play_time_s: 0.0,
            status: String::new(),
            layout_preset: "default".to_string(),
            project_root: None,
            project_files: Vec::new(),
            scene_manifest: SceneManifest::default(),
        };
        workspace.log(
            LogSource::Editor,
            LogLevel::Info,
            "PrismaRev GPUI 编辑器已启动",
        );
        workspace
    }

    /// 从 Web 编辑器的 fixture（`editor/public/demo.json`）恢复全部数据。
    pub fn load_fixture(value: &Value) -> Self {
        let mut workspace = Self::sample();

        if let Some(schema) = value.get("schema").and_then(Schema::from_value) {
            workspace.schema = schema;
        }
        if let Some(presets) = value.get("presets") {
            workspace.presets = EntityPreset::from_list(presets);
        }
        if let Some(assets) = value.get("assetInfos") {
            workspace.assets = crate::assets::parse_assets(assets);
        }
        workspace.tree = AssetTree::build(&workspace.assets);

        // 模板文档（预制体 + 场景）。
        if let Some(template_docs) = value.get("templateDocs").and_then(|v| v.as_object()) {
            if let Some(labels) = value.get("templatesByKind").and_then(|v| v.as_object()) {
                for (kind_name, entries) in labels {
                    let kind = DocKind::parse(kind_name);
                    let Some(items) = entries.as_array() else {
                        continue;
                    };
                    for item in items {
                        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let label = item
                            .get("label")
                            .and_then(|v| v.as_str())
                            .unwrap_or(id)
                            .to_string();
                        if let Some(doc_value) =
                            template_docs.get(kind_name).and_then(|docs| docs.get(id))
                        {
                            if let Ok(mut doc) =
                                serde_json::from_value::<SceneDoc>(doc_value.clone())
                            {
                                doc.normalize();
                                workspace.templates.push(TemplateDoc {
                                    id: id.to_string(),
                                    label,
                                    kind,
                                    doc,
                                });
                            }
                        }
                    }
                }
            }
        }

        // 打开的文档（fixture 的 `docs` 是 path → SceneDoc）。
        let mut opened: Vec<Document> = Vec::new();
        if let Some(docs) = value.get("docs").and_then(|v| v.as_object()) {
            for (path, doc_value) in docs {
                let Ok(mut doc) = serde_json::from_value::<SceneDoc>(doc_value.clone()) else {
                    continue;
                };
                doc.normalize();
                let kind = if path.ends_with(".prefab.json") {
                    DocKind::Prefab
                } else {
                    DocKind::Scene
                };
                opened.push(Document::new(kind, Some(path.clone()), doc));
            }
        }
        if !opened.is_empty() {
            opened.sort_by(|a, b| {
                a.path
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.path.as_deref().unwrap_or(""))
            });
            workspace.docs = opened;
        }

        // 面板注册表来自后端声明的 EditorWindow 列表。
        if let Some(windows) = value.get("windows").and_then(|v| v.as_array()) {
            let mut panels = Vec::new();
            for window in windows {
                let Some(id) = window.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                panels.push(PanelInfo {
                    id: id.to_string(),
                    title: window
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id)
                        .to_string(),
                    menu_path: window
                        .get("menuPath")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    shortcut: window
                        .get("shortcut")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    utility: dock::UTILITY_TABS.contains(&id),
                });
            }
            if !panels.is_empty() {
                workspace.panels = panels;
            }
        }

        workspace.hydrate_documents();
        workspace.active = 0;
        workspace.log(
            LogSource::Editor,
            LogLevel::Info,
            format!(
                "已载入 fixture：{} 个文档 / {} 个组件类型 / {} 个资产",
                workspace.docs.len(),
                workspace.schema.types.len(),
                workspace.assets.len()
            ),
        );
        workspace
    }

    /// 用 schema 的 `defaultJson` 补齐组件缺失字段。
    ///
    /// Web 编辑器先 `from_object` 再回填，所以字段永远齐全；GPUI 直接操作作者 JSON，
    /// 必须在加载后补默认值，否则「拖动一个旧文档里没写过的字段」会静默失败。
    pub fn hydrate_documents(&mut self) {
        let schema = self.schema.clone();
        for document in &mut self.docs {
            hydrate_doc(&schema, &mut document.doc);
        }
        for template in &mut self.templates {
            hydrate_doc(&schema, &mut template.doc);
        }
    }

    // ── 项目文件 ────────────────────────────────────────────────────────

    /// 绑定项目根：扫描 `assets/scenes` / `assets/prefabs`，用磁盘文档替换 fixture 文档。
    /// 返回实际打开的文档数。
    pub fn load_project(&mut self, root: PathBuf) -> io::Result<usize> {
        let files = project::scan_documents(&root);
        let mut opened: Vec<Document> = Vec::new();
        for file in &files {
            match project::read_document(&root, &file.path) {
                Ok(doc) => opened.push(Document::new(file.kind, Some(file.path.clone()), doc)),
                Err(error) => self.log(
                    LogSource::Editor,
                    LogLevel::Warn,
                    format!("读取 {} 失败: {error}", file.path),
                ),
            }
        }

        self.project_root = Some(root);
        self.project_files = files;
        if let Some(root) = self.project_root.clone() {
            self.assets = project::scan_assets(&root);
            self.tree = AssetTree::build(&self.assets);
        }
        match SceneManifest::read(self.project_root.as_deref().expect("project_root 刚刚赋值"))
        {
            Ok(manifest) => self.scene_manifest = manifest,
            Err(error) => self.log(
                LogSource::Editor,
                LogLevel::Warn,
                format!("读取 assets/scenes.toml 失败: {error}"),
            ),
        }
        if !opened.is_empty() {
            self.docs = opened;
            // 优先打开场景（而不是预制体）作为当前文档。
            self.active = self
                .docs
                .iter()
                .position(|document| document.kind == DocKind::Scene)
                .unwrap_or(0);
            self.selection.clear();
            self.expanded.clear();
        }
        self.hydrate_documents();
        let count = self.docs.len();
        self.log(
            LogSource::System,
            LogLevel::Info,
            format!(
                "项目：{} 个文档 / {} 个文件",
                count,
                self.project_files.len()
            ),
        );
        Ok(count)
    }

    /// 从资产面板实例化预制体到场景（Unity 拖预制体到层级）。
    /// 磁盘优先，fixture 模式回退同名模板；返回插入的实体数。
    pub fn instantiate_prefab_asset(
        &mut self,
        relative: &str,
        parent: Option<usize>,
    ) -> io::Result<usize> {
        let prefab = match &self.project_root {
            Some(root) if root.join(relative).exists() => project::read_document(root, relative)?,
            _ => {
                let stem = relative
                    .rsplit('/')
                    .next()
                    .unwrap_or(relative)
                    .trim_end_matches(".prefab.json");
                self.templates
                    .iter()
                    .find(|template| template.kind == DocKind::Prefab && template.id == stem)
                    .map(|template| template.doc.clone())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, format!("找不到预制体 {relative}"))
                    })?
            }
        };

        let parent = parent.filter(|index| *index < self.document().doc.entities.len());
        let inserted = self.edit("实例化预制体", |document| {
            document.doc.instantiate_prefab(&prefab, parent, relative)
        });
        if let Some(root) = inserted.first() {
            self.selection = vec![*root];
            if let Some(id) = self.document().doc.id_of_index(*root) {
                self.expanded.insert(id);
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已实例化 {relative}（{} 个实体）", inserted.len()),
        );
        Ok(inserted.len())
    }

    /// 把当前选中实体子树存成预制体文件（Unity「另存为预制体」）。
    /// 重名自动追加 `-2`、`-3`；不修改当前场景。
    pub fn save_selection_as_prefab(&mut self, name: &str) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法保存预制体")
        })?;
        let index = self
            .primary_selection()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "未选择实体"))?;
        let doc = self.document().doc.extract_subtree(&[index]);
        let stem = project::sanitize_stem(name);
        let mut path = format!("assets/prefabs/{stem}.prefab.json");
        let mut suffix = 2;
        while root.join(&path).exists() {
            path = format!("assets/prefabs/{stem}-{suffix}.prefab.json");
            suffix += 1;
        }
        project::write_document(&root, &path, &doc)?;
        self.refresh_project_files();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已存为预制体 {path}（{} 个实体）", doc.entities.len()),
        );
        Ok(path)
    }

    /// 删除资产文件（只允许 `assets/` 内，场景/预制体走文档流程）。
    /// 引用不会自动清理（与 Web 编辑器一致：引用变成悬空值，由检查器提示）。
    pub fn delete_asset(&mut self, path: &str) -> io::Result<()> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法删除资产")
        })?;
        if !path.starts_with("assets/") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "只允许删除 assets/ 内的文件",
            ));
        }
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "场景 / 预制体请用文档面板",
            ));
        }
        let full = root.join(path);
        if !full.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} 不存在"),
            ));
        }
        std::fs::remove_file(full)?;
        self.refresh_project_files();
        self.log(LogSource::Editor, LogLevel::Info, format!("已删除 {path}"));
        Ok(())
    }

    /// 删除场景 / 预制体文档：磁盘文件、打开文档、项目文件表与 `scenes.toml` 条目一起清理。
    ///
    /// 不可撤销，调用方负责确认；预制体实例会保留（链接悬空，便于发现）。
    pub fn delete_document(&mut self, path: &str) -> io::Result<()> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法删除文档")
        })?;
        if !path.starts_with("assets/")
            || !(path.ends_with(".scene.json") || path.ends_with(".prefab.json"))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "只允许删除 assets/ 下的 .scene.json / .prefab.json",
            ));
        }
        let full = root.join(path);
        if !full.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} 不存在"),
            ));
        }
        std::fs::remove_file(full)?;

        let removed = self
            .docs
            .iter()
            .position(|document| document.path.as_deref() == Some(path));
        self.docs
            .retain(|document| document.path.as_deref() != Some(path));
        if let Some(index) = removed {
            if index < self.active {
                self.active -= 1;
            }
        }
        self.active = self.active.min(self.docs.len().saturating_sub(1));
        self.selection.clear();
        self.expanded.clear();
        if self.docs.is_empty() {
            self.new_document(DocKind::Scene, None);
        }

        let doc_relative = path.trim_start_matches("assets/");
        if let Some(position) = self.scene_manifest.scenes.iter().position(|entry| {
            crate::manifest::doc_relative_path(&entry.path).as_deref() == Some(doc_relative)
        }) {
            self.scene_manifest.remove(position);
            self.persist_scene_manifest();
        }
        self.project_files.retain(|file| file.path != path);
        self.log(LogSource::Editor, LogLevel::Info, format!("已删除 {path}"));
        Ok(())
    }

    /// 资产重命名 / 移动：先改磁盘文件，再改写所有打开文档里的精确路径引用，
    /// 并同步文档自身的路径。返回 `(替换次数, 受影响文档)`。
    pub fn rename_asset(&mut self, from: &str, to: &str) -> io::Result<(usize, Vec<String>)> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法重命名资产")
        })?;
        if from.ends_with(".scene.json") || from.ends_with(".prefab.json") {
            // 文档走「另存为 / 打开」路径，避免和未保存内容打架。
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "场景 / 预制体请用另存为或文档面板",
            ));
        }
        project::rename_path(&root, from, to)?;

        let mut references = 0;
        let mut affected = Vec::new();
        for document in &mut self.docs {
            let count = document.doc.rewrite_reference(from, to);
            if document.path.as_deref() == Some(from) {
                document.path = Some(to.to_string());
            }
            if count > 0 {
                document.dirty = true;
                references += count;
                affected.push(document.title());
            }
        }
        self.refresh_project_files();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!(
                "已重命名 {from} → {to}（改写 {references} 处引用，{} 个文档）",
                affected.len()
            ),
        );
        Ok((references, affected))
    }

    pub fn refresh_project_files(&mut self) {
        if let Some(root) = &self.project_root {
            self.project_files = project::scan_documents(root);
        }
    }

    /// 打开（或切到）磁盘上的文档。
    pub fn open_project_file(&mut self, relative: &str) -> io::Result<()> {
        if let Some(index) = self
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some(relative))
        {
            self.set_active(index);
            return Ok(());
        }
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "没有绑定项目根（纯 fixture 模式）",
            ));
        };
        let doc = project::read_document(&root, relative)?;
        let kind = if relative.ends_with(".prefab.json") {
            DocKind::Prefab
        } else {
            DocKind::Scene
        };
        self.docs
            .push(Document::new(kind, Some(relative.to_string()), doc));
        self.active = self.docs.len() - 1;
        self.selection.clear();
        self.expanded.clear();
        self.hydrate_documents();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已打开 {relative}"),
        );
        Ok(())
    }

    /// 保存当前文档；未命名文档落到 `assets/scenes|prefabs` 下的默认路径。
    pub fn save_active(&mut self) -> io::Result<String> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法保存"))?;

        let (path, kind, doc) = {
            let document = self.document();
            let path = match &document.path {
                Some(path) => path.clone(),
                None => project::default_save_path(&root, document.kind, &document.title()),
            };
            (path, document.kind, document.doc.clone())
        };
        let _ = kind;
        project::write_document(&root, &path, &doc)?;
        {
            let document = self.document_mut();
            document.path = Some(path.clone());
            document.dirty = false;
        }
        self.refresh_project_files();
        self.log(LogSource::Editor, LogLevel::Info, format!("已保存 {path}"));
        Ok(path)
    }

    /// 自动保存用：只在文档有未保存修改时才写盘。
    pub fn save_active_if_dirty(&mut self) -> Option<io::Result<String>> {
        if !self.document().dirty {
            return None;
        }
        Some(self.save_active())
    }

    /// 另存为（路径相对项目根；空串/非法输入返回错误）。
    pub fn save_active_as(&mut self, input: &str) -> io::Result<String> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根，无法保存"))?;
        let kind = self.document().kind;
        let path = project::normalize_save_path(kind, input);
        if path.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "路径为空"));
        }
        if !path.starts_with("assets/") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "只允许写到 assets/ 下",
            ));
        }
        let doc = self.document().doc.clone();
        project::write_document(&root, &path, &doc)?;
        {
            let document = self.document_mut();
            document.path = Some(path.clone());
            document.dirty = false;
        }
        self.refresh_project_files();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已另存为 {path}"),
        );
        Ok(path)
    }

    /// 重新从磁盘读取当前文档（丢弃未保存修改）。
    pub fn reload_active(&mut self) -> io::Result<()> {
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根"));
        };
        let path = self
            .document()
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "文档尚未保存"))?;
        let doc = project::read_document(&root, &path)?;
        let document = self.document_mut();
        document.doc = doc;
        document.undo.clear();
        document.dirty = false;
        self.hydrate_documents();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已重新读取 {path}"),
        );
        Ok(())
    }

    pub fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub fn save_suggestion(&self) -> String {
        let document = self.document();
        match &document.path {
            Some(path) => path.clone(),
            None => match &self.project_root {
                Some(root) => project::default_save_path(root, document.kind, &document.title()),
                None => format!("{}/{}", document.kind.folder(), document.title()),
            },
        }
    }

    // ── 场景清单（assets/scenes.toml） ───────────────────────────────────

    /// 落盘失败只记日志：清单编辑不应打断编辑器的其它操作。
    fn persist_scene_manifest(&mut self) {
        let Some(root) = self.project_root.clone() else {
            return;
        };
        if let Err(error) = self.scene_manifest.write(&root) {
            self.log(
                LogSource::Editor,
                LogLevel::Error,
                format!("写入 assets/scenes.toml 失败: {error}"),
            );
        }
    }

    pub fn reload_scene_manifest(&mut self) -> io::Result<()> {
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "没有绑定项目根"));
        };
        self.scene_manifest = SceneManifest::read(&root)?;
        Ok(())
    }

    pub fn set_scene_enabled(&mut self, index: usize, enabled: bool) {
        if self.scene_manifest.set_enabled(index, enabled) {
            self.persist_scene_manifest();
        }
    }

    /// 上移 / 下移一条，返回新下标。
    pub fn move_scene_entry(&mut self, index: usize, delta: i32) -> Option<usize> {
        let moved = self.scene_manifest.move_entry(index, delta);
        if moved.is_some() {
            self.persist_scene_manifest();
        }
        moved
    }

    pub fn remove_scene_entry(&mut self, index: usize) -> bool {
        let removed = self.scene_manifest.remove(index).is_some();
        if removed {
            self.persist_scene_manifest();
        }
        removed
    }

    /// 把磁盘上的场景文档加入清单；已存在 / 无项目根时返回 `None`。
    pub fn add_scene_document(&mut self, doc_relative: &str) -> Option<usize> {
        self.project_root.as_ref()?;
        let added = self.scene_manifest.add_document(doc_relative);
        if added.is_some() {
            self.persist_scene_manifest();
        }
        added
    }

    /// `assets/scenes` 下尚未写进清单的场景文档（相对 `assets/`）。
    pub fn available_scene_documents(&self) -> Vec<String> {
        let mut docs: Vec<String> = self
            .project_files
            .iter()
            .filter(|file| file.kind == DocKind::Scene)
            .map(|file| file.path.trim_start_matches("assets/").to_string())
            .filter(|relative| !self.scene_manifest.contains_document(relative))
            .collect();
        docs.sort();
        docs
    }

    // ── 文档 ─────────────────────────────────────────────────────────────

    pub fn document(&self) -> &Document {
        &self.docs[self.active.min(self.docs.len() - 1)]
    }

    pub fn document_mut(&mut self) -> &mut Document {
        let index = self.active.min(self.docs.len() - 1);
        &mut self.docs[index]
    }

    pub fn active_index(&self) -> usize {
        self.active.min(self.docs.len().saturating_sub(1))
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.docs.len() {
            self.active = index;
            self.selection.clear();
            self.expanded.clear();
        }
    }

    pub fn open_path(&mut self, path: &str) {
        match self
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some(path))
        {
            Some(index) => self.set_active(index),
            None => {
                // 模板可以直接“实例化”成一个新文档。
                let template = self
                    .templates
                    .iter()
                    .find(|template| template.id == path)
                    .cloned();
                if let Some(template) = template {
                    let mut template = template;
                    hydrate_doc(&self.schema, &mut template.doc);
                    let document = Document::new(
                        template.kind,
                        Some(format!(
                            "{}/{}{}",
                            template.kind.folder(),
                            template.id,
                            template.kind.suffix()
                        )),
                        template.doc,
                    );
                    self.docs.push(document);
                    self.active = self.docs.len() - 1;
                    self.selection.clear();
                    self.log(
                        LogSource::Editor,
                        LogLevel::Info,
                        format!("已打开模板 {path}"),
                    );
                }
            }
        }
    }

    pub fn new_document(&mut self, kind: DocKind, template_id: Option<&str>) {
        let template = template_id
            .and_then(|id| self.templates.iter().find(|template| template.id == id))
            .filter(|template| template.kind == kind)
            .cloned();
        let mut doc = template
            .as_ref()
            .map(|template| template.doc.clone())
            .unwrap_or_default();
        hydrate_doc(&self.schema, &mut doc);
        let path = template
            .as_ref()
            .map(|template| format!("{}/{}{}", kind.folder(), template.id, kind.suffix()));
        self.docs.push(Document::new(kind, path, doc));
        self.active = self.docs.len() - 1;
        self.selection.clear();
        self.expanded.clear();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("新建{}", kind.folder()),
        );
    }

    pub fn close_document(&mut self, index: usize) {
        if self.docs.len() <= 1 || index >= self.docs.len() {
            return;
        }
        self.docs.remove(index);
        if self.active >= self.docs.len() {
            self.active = self.docs.len() - 1;
        }
        self.selection.clear();
    }

    pub fn mark_saved(&mut self) {
        let title = {
            let document = self.document_mut();
            document.dirty = false;
            document.title()
        };
        self.log(LogSource::Editor, LogLevel::Info, format!("已保存 {title}"));
    }

    /// 结构性编辑：自动快照 → 执行 → 入撤销栈。
    pub fn edit<T>(
        &mut self,
        label: impl Into<String>,
        mutate: impl FnOnce(&mut Document) -> T,
    ) -> T {
        let before = self.document().doc.clone();
        let label = label.into();
        let result = {
            let document = self.document_mut();
            mutate(document)
        };
        let after = self.document().doc.clone();
        if before != after {
            let document = self.document_mut();
            document.undo.push_snapshot(label, before, &after);
            document.dirty = true;
        }
        result
    }

    // ── 选择 / 层级 ─────────────────────────────────────────────────────

    pub fn select(&mut self, index: usize, extend: bool) {
        if !extend {
            self.selection.clear();
        }
        if let Some(position) = self.selection.iter().position(|item| *item == index) {
            if extend {
                self.selection.remove(position);
            }
        } else {
            self.selection.push(index);
        }
        self.selection.sort_unstable();
    }

    pub fn select_range(&mut self, anchor: usize, index: usize) {
        let (start, end) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        self.selection = (start..=end).collect();
    }

    /// 框选：`additive = false` 覆盖选择，`true` 并入。
    pub fn select_many(&mut self, indices: &[usize], additive: bool) {
        if !additive {
            self.selection.clear();
        }
        for index in indices {
            if !self.selection.contains(index) {
                self.selection.push(*index);
            }
        }
        self.selection.sort_unstable();
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn primary_selection(&self) -> Option<usize> {
        self.selection.first().copied()
    }

    pub fn toggle_expanded(&mut self, index: usize) {
        if let Some(id) = self.document().doc.id_of_index(index) {
            if !self.expanded.insert(id) {
                self.expanded.remove(&id);
            }
        }
    }

    pub fn expand_all(&mut self) {
        let ids: Vec<u32> = self
            .document()
            .doc
            .entities
            .iter()
            .filter_map(|entity| entity.id)
            .collect();
        self.expanded.extend(ids);
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    pub fn add_entity(&mut self, preset_id: &str) {
        let preset = self
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned();
        let schema = self.schema.clone();
        let parent = self.primary_selection();
        self.edit("新建实体", |document| {
            let entity = build_entity(&schema, preset.as_ref(), parent, &document.doc);
            document.doc.add_entity(entity);
        });
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let indices = self.selection.clone();
        self.edit("删除实体", |document| {
            document.doc.remove_entities(&indices);
        });
        self.selection.clear();
    }

    /// 顶层选中实体：去掉“父级也在选中集合里”的项，避免复制整棵子树两次。
    pub fn selection_roots(&self) -> Vec<usize> {
        let doc = &self.document().doc;
        let ids: HashSet<u32> = self
            .selection
            .iter()
            .filter_map(|index| doc.entities.get(*index).and_then(|entity| entity.id))
            .collect();
        let mut roots = Vec::new();
        for index in &self.selection {
            let Some(entity) = doc.entities.get(*index) else {
                continue;
            };
            let mut parent = entity.parent;
            let mut nested = false;
            let mut depth = 0;
            while let Some(parent_id) = parent {
                if ids.contains(&parent_id) {
                    nested = true;
                    break;
                }
                depth += 1;
                if depth > 64 {
                    break;
                }
                parent = doc
                    .index_of_id(parent_id)
                    .and_then(|parent_index| doc.entities.get(parent_index))
                    .and_then(|parent| parent.parent);
            }
            if !nested {
                roots.push(*index);
            }
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }

    /// 复制选中实体到内部剪贴板（不写系统剪贴板）。返回实体数。
    pub fn copy_selection(&mut self) -> usize {
        let roots = self.selection_roots();
        if roots.is_empty() {
            return 0;
        }
        let trees: Vec<SceneDoc> = roots
            .iter()
            .map(|index| {
                self.document()
                    .doc
                    .extract_subtree_preserving_links(&[*index])
            })
            .collect();
        let count: usize = trees.iter().map(|tree| tree.entities.len()).sum();
        self.entity_clipboard = Some(EntityClipboard { trees, cut: false });
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("已复制 {count} 个实体"),
        );
        count
    }

    /// 剪切 = 复制 + 删除（一次删除撤销步）。
    pub fn cut_selection(&mut self) -> usize {
        let count = self.copy_selection();
        if count > 0 {
            if let Some(clipboard) = &mut self.entity_clipboard {
                clipboard.cut = true;
            }
            self.delete_selection();
        }
        count
    }

    /// 粘贴实体剪贴板：`parent = None` 落到根级，否则挂到该实体下。
    /// 返回粘贴出来的根实体下标（并设为当前选择）。
    pub fn paste_entities(&mut self, parent: Option<usize>) -> Vec<usize> {
        let Some(clipboard) = self.entity_clipboard.clone() else {
            return Vec::new();
        };
        if clipboard.trees.is_empty() {
            return Vec::new();
        }
        let parent_valid = parent.filter(|index| *index < self.document().doc.entities.len());
        let roots = self.edit("粘贴实体", |document| {
            let mut roots = Vec::new();
            for tree in &clipboard.trees {
                let inserted = document.doc.instantiate_prefab(tree, parent_valid, "");
                if let Some(root) = inserted.first() {
                    roots.push(*root);
                }
            }
            roots
        });
        if roots.is_empty() {
            return roots;
        }
        self.selection = roots.clone();
        for index in &roots {
            if let Some(id) = self.document().doc.id_of_index(*index) {
                self.expanded.insert(id);
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已粘贴 {} 个实体", roots.len()),
        );
        roots
    }

    pub fn duplicate_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let roots = self.selection_roots();
        let mut inserted = self.edit("复制实体", |document| {
            let mut inserted = Vec::new();
            // 从后往前复制，前面的下标不会被新追加的实体挤动。
            for index in roots.iter().rev() {
                if let Some(new_index) = document.doc.duplicate_subtree(*index) {
                    inserted.push(new_index);
                }
            }
            inserted
        });
        if inserted.is_empty() {
            return;
        }
        inserted.sort_unstable();
        self.selection = inserted.clone();
        for index in &inserted {
            if let Some(id) = self.document().doc.id_of_index(*index) {
                self.expanded.insert(id);
            }
        }
    }

    pub fn reparent(&mut self, index: usize, new_parent: Option<usize>, position: Option<usize>) {
        let result = self.edit("调整层级", |document| {
            document.doc.reparent(index, new_parent, position).is_ok()
        });
        if !result {
            self.log(
                LogSource::Editor,
                LogLevel::Warn,
                "层级调整被拒绝（自父 / 成环）",
            );
        }
    }

    pub fn set_active_entity(&mut self, index: usize, active: bool) {
        self.edit(
            if active {
                "启用实体"
            } else {
                "禁用实体"
            },
            |document| {
                document.doc.set_active(index, active);
            },
        );
    }

    /// 预制体实例的还原 / 同步（Unity `PrefabUtility.RevertPrefabInstance`）。
    ///
    /// 用预制体当前内容覆盖实例子树：保留实例根的名字与父级，重新生成根上的链接组件。
    /// 数据来源优先磁盘（项目根 + 链接路径），纯 fixture 模式下回退到同名模板。
    pub fn revert_prefab_instance(&mut self, entity_id: u32) -> io::Result<()> {
        let prefab = self.prefab_document_for(entity_id)?;
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "实例不存在"));
        };
        let (name, parent_id, link) = {
            let entity = &self.document().doc.entities[index];
            (
                entity.display_name(),
                entity.parent,
                entity.prefab_link().unwrap_or_default().to_string(),
            )
        };

        self.edit("还原预制体实例", |document| {
            document.doc.remove_entities(&[index]);
            let parent = parent_id.and_then(|id| document.doc.index_of_id(id));
            let inserted = document.doc.instantiate_prefab(&prefab, parent, &link);
            if let Some(root) = inserted.first() {
                let root = *root;
                if let Some(entity) = document.doc.entities.get_mut(root) {
                    entity.name = Some(name.clone());
                }
            }
        });
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已还原预制体实例 {name}"),
        );
        Ok(())
    }

    /// 同步所有同源实例（按实体 id 从后往前处理，避免下标漂移）。
    pub fn sync_prefab_instances(&mut self, prefab_path: &str) -> io::Result<usize> {
        let ids: Vec<u32> = self
            .document()
            .doc
            .entities
            .iter()
            .filter(|entity| entity.prefab_link() == Some(prefab_path))
            .filter_map(|entity| entity.id)
            .collect();
        let mut synced = 0;
        for id in ids.into_iter().rev() {
            if self.revert_prefab_instance(id).is_ok() {
                synced += 1;
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("同步 {synced} 个 {prefab_path} 实例"),
        );
        Ok(synced)
    }

    /// 实例对应的预制体文档：磁盘优先，fixture 模式回退到模板（按文件名）。
    pub fn prefab_document_for(&self, entity_id: u32) -> io::Result<SceneDoc> {
        let path = self
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.id == Some(entity_id))
            .and_then(|entity| entity.prefab_link())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "不是预制体实例"))?
            .to_string();

        if let Some(root) = &self.project_root {
            return project::read_document(root, &path);
        }

        let stem = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .trim_end_matches(".prefab.json");
        self.templates
            .iter()
            .find(|template| template.kind == DocKind::Prefab && template.id == stem)
            .map(|template| template.doc.clone())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("找不到预制体 {path}（纯 fixture 模式下需要同名模板）"),
                )
            })
    }

    /// 解除预制体实例链接（移除编辑器元数据组件，场景内容保持不变）。
    pub fn unlink_prefab(&mut self, index: usize) -> bool {
        let linked = self
            .document()
            .doc
            .entities
            .get(index)
            .map(|entity| entity.has_component(PREFAB_LINK_COMPONENT))
            .unwrap_or(false);
        if !linked {
            return false;
        }
        self.edit("解除预制体链接", |document| {
            document.doc.remove_component(index, PREFAB_LINK_COMPONENT);
        });
        true
    }

    /// 内联重命名的实时预览：只改文档，不产生撤销步（避免每敲一个字符一步）。
    pub fn preview_rename(&mut self, index: usize, name: &str) {
        if let Some(entity) = self.document_mut().doc.entities.get_mut(index) {
            if entity.name.as_deref() != Some(name) {
                entity.name = Some(name.to_string());
                self.document_mut().dirty = true;
            }
        }
    }

    /// 重命名结束：用开始编辑前的快照补一个撤销步（内容没变则不记）。
    pub fn finish_rename(&mut self, base: SceneDoc) {
        let after = self.document().doc.clone();
        if base != after {
            self.document_mut()
                .undo
                .push_snapshot("重命名实体", base, &after);
            self.document_mut().dirty = true;
        }
    }

    pub fn rename_entity(&mut self, index: usize, name: &str) {
        self.edit("重命名实体", |document| {
            document.doc.rename(index, name);
        });
    }

    // ── 组件 ────────────────────────────────────────────────────────────

    pub fn component_types(&self) -> Vec<&TypeSchema> {
        self.schema
            .components()
            .into_iter()
            .filter(|component| {
                self.primary_selection()
                    .and_then(|index| self.document().doc.entities.get(index))
                    .map(|entity| !entity.has_component(&component.name))
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn add_component(&mut self, component_name: &str) {
        let Some(index) = self.primary_selection() else {
            return;
        };
        let Some(component) = self.schema.component(component_name) else {
            return;
        };
        let name = component.name.clone();
        let value = component_value_with_overrides(&component.default_value(), None);
        self.edit("添加组件", |document| {
            let entity = &mut document.doc.entities[index];
            entity.components.insert(name, value);
        });
    }

    pub fn remove_component(&mut self, component_name: &str) {
        let Some(index) = self.primary_selection() else {
            return;
        };
        if component_name == "Transform" {
            return;
        }
        self.edit("移除组件", |document| {
            document.doc.remove_component(index, component_name);
        });
    }

    pub fn copy_component(&mut self, component_name: &str) {
        let Some(index) = self.primary_selection() else {
            return;
        };
        self.copy_component_at(index, component_name);
    }

    pub fn copy_component_at(&mut self, index: usize, component_name: &str) {
        let Some(value) = self.document().doc.entities[index].component(component_name) else {
            return;
        };
        self.clipboard = Some(ComponentClipboard {
            component: component_name.to_string(),
            value: value.clone(),
        });
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("已复制组件 {component_name}"),
        );
    }

    pub fn paste_component(&mut self) {
        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };
        let Some(index) = self.primary_selection() else {
            return;
        };
        self.edit("粘贴组件", |document| {
            document.doc.entities[index]
                .components
                .insert(clipboard.component.clone(), clipboard.value.clone());
        });
    }

    /// 多选时都能添加的组件（交集）：检查器「多对象编辑」只显示公共组件。
    pub fn common_components(&self, indices: &[usize]) -> Vec<TypeSchema> {
        let doc = &self.document().doc;
        let first = indices
            .iter()
            .filter_map(|index| doc.entities.get(*index))
            .next();
        let Some(first) = first else {
            return Vec::new();
        };
        let mut common: Vec<TypeSchema> = first
            .components
            .keys()
            .filter_map(|name| self.schema.component(name).cloned())
            .collect();
        for index in indices.iter().skip(1) {
            let Some(entity) = doc.entities.get(*index) else {
                continue;
            };
            common.retain(|component| entity.has_component(&component.name));
        }
        common
    }

    /// 多选字段写入：一次快照、一个撤销步，应用到每个存在的实体。
    /// 返回实际写入的实体数。
    pub fn set_field_multi(
        &mut self,
        entity_ids: &[u32],
        component: &str,
        path: &str,
        value: Value,
    ) -> usize {
        if entity_ids.is_empty() {
            return 0;
        }
        if entity_ids.len() == 1 {
            self.set_field(entity_ids[0], component, path, value);
            return 1;
        }
        let component_name = component.to_string();
        let path = path.to_string();
        self.edit("批量修改字段", |document| {
            let mut applied = 0;
            for id in entity_ids {
                let Some(index) = document.doc.index_of_id(*id) else {
                    continue;
                };
                let Some(root) = document.doc.entities[index]
                    .components
                    .get_mut(&component_name)
                else {
                    continue;
                };
                if crate::schema::set_at_path(root, &path, value.clone()) {
                    applied += 1;
                }
            }
            applied
        })
    }

    /// 从多选实体上移除同一个组件（不存在的跳过）。
    pub fn remove_component_multi(&mut self, indices: &[usize], component: &str) {
        let indices = indices.to_vec();
        let component_name = component.to_string();
        self.edit("批量移除组件", |document| {
            for index in &indices {
                document.doc.remove_component(*index, &component_name);
            }
        });
    }

    /// 检查器字段写入：反射写 → 撤销步（同字段合并）。
    pub fn set_field(&mut self, entity_id: u32, component: &str, path: &str, value: Value) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let Some(root) = self.document().doc.entities[index]
            .components
            .get(component)
        else {
            return;
        };
        let Some(before) = crate::schema::get_at(root, path) else {
            return;
        };
        if before == value {
            return;
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document.undo.push_field(
            &mut document.doc,
            entity_id,
            component,
            path,
            before,
            value,
            now_ms,
        );
        document.dirty = true;
    }

    /// 绕世界轴旋转实体（旋转 gizmo）；四元数 `q_axis * q_old`，按实体合并撤销。
    pub fn rotate_entity(&mut self, entity_id: u32, axis: usize, delta_radians: f32) {
        if !delta_radians.is_finite() || delta_radians.abs() < 1e-6 {
            return;
        }
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        // 世界空间：左乘（绕世界轴）；局部空间：右乘（绕实体自身轴）。
        let after_rotation = match self.gizmo_space {
            GizmoSpace::World => quat_mul(quat_axis_angle(axis, delta_radians), before.rotation),
            GizmoSpace::Local => quat_mul(before.rotation, quat_axis_angle(axis, delta_radians)),
        };
        let after = TransformJson {
            rotation: quat_normalize(after_rotation),
            ..before
        };
        let now = now_ms();
        let document = self.document_mut();
        document
            .undo
            .push_transform(&mut document.doc, entity_id, before, after, now);
        document.dirty = true;
    }

    /// 多选 gizmo 旋转：每个实体绕自身位置旋转（局部模式各用自身轴），
    /// 整次拖动只产生一个撤销步。
    pub fn rotate_entities(&mut self, entity_ids: &[u32], axis: usize, delta_radians: f32) {
        if entity_ids.is_empty() || !delta_radians.is_finite() || delta_radians.abs() < 1e-6 {
            return;
        }
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let space = self.gizmo_space;
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let after_rotation = match space {
                GizmoSpace::World => {
                    quat_mul(quat_axis_angle(axis, delta_radians), before.rotation)
                }
                GizmoSpace::Local => {
                    quat_mul(before.rotation, quat_axis_angle(axis, delta_radians))
                }
            };
            changes.push((
                *entity_id,
                before,
                TransformJson {
                    rotation: quat_normalize(after_rotation),
                    ..before
                },
            ));
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        if changes.len() == 1 {
            let (entity_id, before, after) = changes[0];
            document
                .undo
                .push_transform(&mut document.doc, entity_id, before, after, now);
        } else {
            let key = format!("rotate-group/{ids:?}");
            document
                .undo
                .push_transform_batch(&mut document.doc, changes, key, now);
        }
        document.dirty = true;
    }

    /// 选中实体的 gizmo 基准（世界空间三轴）。世界模式恒为单位基。
    pub fn gizmo_basis(&self) -> [[f32; 3]; 3] {
        match self.gizmo_space {
            GizmoSpace::World => crate::viewport_math::IDENTITY_BASIS,
            GizmoSpace::Local => self
                .primary_selection()
                .map(|index| basis_from_quat(world_rotation(&self.document().doc, index)))
                .unwrap_or(crate::viewport_math::IDENTITY_BASIS),
        }
    }

    pub fn toggle_gizmo_space(&mut self) {
        self.gizmo_space = self.gizmo_space.toggled();
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("gizmo 坐标空间：{}", self.gizmo_space.label()),
        );
    }

    pub fn toggle_grid(&mut self) {
        self.show_grid = !self.show_grid;
    }

    /// 把世界空间位移换算到父级空间后加到实体 `position` 上（gizmo 拖动用）。
    pub fn translate_entity_world(&mut self, entity_id: u32, delta_world: [f32; 3]) {
        if delta_world.iter().all(|value| value.abs() < 1e-6) {
            return;
        }
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let parent_rotation = parent_world_rotation(&self.document().doc, index);
        let delta_local = quat_rotate(quat_conjugate(parent_rotation), delta_world);
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        for (position, delta) in after.position.iter_mut().zip(delta_local) {
            *position += delta;
        }
        let now = now_ms();
        let document = self.document_mut();
        document
            .undo
            .push_transform(&mut document.doc, entity_id, before, after, now);
        document.dirty = true;
    }

    /// 多选 gizmo 平移：每个实体各自换算父空间位移，但整次拖动只产生一个撤销步。
    pub fn translate_entities_world(&mut self, entity_ids: &[u32], delta_world: [f32; 3]) {
        if entity_ids.is_empty() || delta_world.iter().all(|value| value.abs() < 1e-6) {
            return;
        }
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let parent_rotation = parent_world_rotation(doc, index);
            let delta_local = quat_rotate(quat_conjugate(parent_rotation), delta_world);
            let before = doc.entities[index].transform;
            let mut after = before;
            for (position, delta) in after.position.iter_mut().zip(delta_local) {
                *position += delta;
            }
            changes.push((*entity_id, before, after));
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        if changes.len() == 1 {
            let (entity_id, before, after) = changes[0];
            document
                .undo
                .push_transform(&mut document.doc, entity_id, before, after, now);
        } else {
            let key = format!("transform-group/{ids:?}");
            document
                .undo
                .push_transform_batch(&mut document.doc, changes, key, now);
        }
        document.dirty = true;
    }

    /// 缩放 gizmo：只改一个轴（下限 0.01，避免退化矩阵）。
    pub fn set_scale_axis(&mut self, entity_id: u32, axis: usize, value: f32) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        after.scale[axis] = value.clamp(0.01, 1000.0);
        if before == after {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document
            .undo
            .push_transform(&mut document.doc, entity_id, before, after, now);
        document.dirty = true;
    }

    /// 多选 gizmo 缩放：`targets` 是 (实体 id, 拖动开始时的轴缩放值)，`ratio` 为整体倍数。
    /// 每个实体保留自己的起始比例，整次拖动只产生一个撤销步。
    pub fn scale_entities_axis(&mut self, targets: &[(u32, f32)], axis: usize, ratio: f32) {
        if targets.is_empty() || !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        let mut sorted: Vec<(u32, f32)> = targets.to_vec();
        sorted.sort_by_key(|(entity_id, _)| *entity_id);
        sorted.dedup_by_key(|(entity_id, _)| *entity_id);
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for (entity_id, start_value) in &sorted {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            after.scale[axis] = (*start_value * ratio).clamp(0.01, 1000.0);
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let ids: Vec<u32> = sorted.iter().map(|(entity_id, _)| *entity_id).collect();
        let now = now_ms();
        let document = self.document_mut();
        if changes.len() == 1 {
            let (entity_id, before, after) = changes[0];
            document
                .undo
                .push_transform(&mut document.doc, entity_id, before, after, now);
        } else {
            let key = format!("scale-group/{ids:?}");
            document
                .undo
                .push_transform_batch(&mut document.doc, changes, key, now);
        }
        document.dirty = true;
    }

    /// 检查器里拖动 Transform 的一个分量（按实体合并撤销步）。
    pub fn set_transform_axis(&mut self, entity_id: u32, field: &str, axis: usize, value: f32) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        match field {
            "position" => after.position[axis] = value,
            "rotation" => after.rotation[axis] = value,
            "scale" => after.scale[axis] = value,
            _ => return,
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document
            .undo
            .push_transform(&mut document.doc, entity_id, before, after, now_ms);
        document.dirty = true;
    }

    // ── 撤销 ────────────────────────────────────────────────────────────

    pub fn undo(&mut self) {
        let document = self.document_mut();
        if document.undo.undo(&mut document.doc).is_some() {
            document.dirty = true;
        }
    }

    pub fn redo(&mut self) {
        let document = self.document_mut();
        if document.undo.redo(&mut document.doc).is_some() {
            document.dirty = true;
        }
    }

    pub fn can_undo(&self) -> bool {
        self.document().undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.document().undo.can_redo()
    }

    // ── 面板 / 布局 ─────────────────────────────────────────────────────

    pub fn panel(&self, id: &str) -> Option<&PanelInfo> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn panel_title(&self, id: &str) -> String {
        match id {
            "sceneView" => self.i18n.t1("menu.sceneView"),
            "gameView" => self.i18n.t1("menu.gameView"),
            "docs" => self.i18n.t1("menu.docs"),
            "hierarchy" => self.i18n.t1("menu.hierarchy"),
            "assets" => self.i18n.t1("menu.assets"),
            "inspector" => self.i18n.t1("menu.inspector"),
            "log" => self.i18n.t1("menu.log"),
            "renderGraph" => self.i18n.t1("menu.renderGraph"),
            "projectSettings" => self.i18n.t1("menu.projectSettings"),
            "preferences" => self.i18n.t1("menu.preferences"),
            "theme" => self.i18n.t1("menu.theme"),
            "build" => self.i18n.t1("menu.buildPanel"),
            "tools" => self.i18n.t1("menu.tools"),
            "shortcuts" => self.i18n.t1("menu.shortcuts"),
            "about" => self.i18n.t1("menu.about"),
            other => self
                .panel(other)
                .map(|panel| panel.title.clone())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    pub fn known_panels(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.panels.iter().map(|panel| panel.id.clone()).collect();
        for id in dock::UTILITY_TABS {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
        ids
    }

    pub fn show_panel(&mut self, id: &str) {
        self.dock = dock::show_panel(&self.dock, id);
    }

    /// `window/toggle/<id>`：已经在某个 leaf 里就切到它，否则放回默认锚点。
    pub fn toggle_panel(&mut self, id: &str) {
        if let Some(leaf) = find_leaf(&self.dock, id) {
            let leaf_id = leaf.id().to_string();
            self.dock = dock::activate_tab(&self.dock, &leaf_id, id);
            return;
        }
        self.dock = dock::show_panel(&self.dock, id);
    }

    pub fn activate_panel(&mut self, leaf_id: &str, tab: &str) {
        self.dock = dock::activate_tab(&self.dock, leaf_id, tab);
    }

    /// 拖拽结束：中心 = 合并成 tab；边缘 = 新开分屏。
    pub fn dock_tab(&mut self, tab: &str, target_leaf: &str, zone: DockZone) {
        self.dock = insert_panel(&self.dock, target_leaf, tab, zone, 0.5, None);
        self.floating.retain(|panel| panel.tab != tab);
    }

    /// 拖到主窗口左/右/底边。
    pub fn dock_tab_at_edge(&mut self, tab: &str, edge: DockEdge) {
        self.dock = dock::dock_panel_at_edge(Some(&self.dock), tab, edge, 0.25);
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn float_panel(&mut self, tab: &str, x: f32, y: f32) {
        self.dock = remove_panel(&self.dock, tab).unwrap_or_else(|| leaf([tab]));
        let z = self.floating.iter().map(|panel| panel.z).max().unwrap_or(0) + 1;
        let panel = dock::clamp_float_panel(
            FloatPanel {
                tab: tab.to_string(),
                x,
                y,
                width: 340.0,
                height: 380.0,
                z,
            },
            1600.0,
            900.0,
        );
        self.floating.push(panel);
    }

    pub fn close_floating(&mut self, tab: &str) {
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn raise_floating(&mut self, tab: &str) {
        let z = self.floating.iter().map(|panel| panel.z).max().unwrap_or(0) + 1;
        if let Some(panel) = self.floating.iter_mut().find(|panel| panel.tab == tab) {
            panel.z = z;
        }
    }

    pub fn set_layout_preset(&mut self, id: &str) {
        if let Some(node) = dock::layout_preset(id) {
            self.dock = node;
            self.floating.clear();
            self.layout_preset = id.to_string();
        }
    }

    pub fn layout_json(&self) -> Value {
        dock::DockLayout {
            version: dock::DockLayout::VERSION,
            dock: self.dock.clone(),
            floating: self.floating.clone(),
        }
        .to_json()
    }

    // ── 资产 ────────────────────────────────────────────────────────────

    pub fn filtered_assets(&self) -> Vec<&AssetInfo> {
        crate::assets::filter_assets(&self.assets, &self.asset_query)
    }

    /// 检查器「选择资产」候选：按字段名推断资产类别，同类优先（全部按路径排序）。
    pub fn asset_picker_candidates(&self, field: &str) -> Vec<AssetInfo> {
        let leaf = field
            .rsplit('.')
            .next()
            .unwrap_or(field)
            .to_ascii_lowercase();
        let kind = if leaf == "env_map"
            || leaf.contains("hdr")
            || leaf.contains("env")
            || leaf.contains("sky")
        {
            Some(AssetKind::Env)
        } else if leaf.contains("mesh") || leaf.contains("model") {
            Some(AssetKind::Model)
        } else if leaf.contains("sprite")
            || leaf.contains("texture")
            || leaf.contains("icon")
            || leaf.contains("tex")
        {
            Some(AssetKind::Texture)
        } else if leaf.contains("audio") || leaf.contains("sound") || leaf.contains("music") {
            Some(AssetKind::Audio)
        } else if leaf.contains("material") {
            Some(AssetKind::Material)
        } else if leaf.contains("prefab") {
            Some(AssetKind::Prefab)
        } else if leaf.contains("scene") {
            Some(AssetKind::Scene)
        } else {
            None
        };
        let mut candidates: Vec<AssetInfo> = self
            .assets
            .iter()
            .filter(|asset| kind.is_none_or(|kind| asset.kind == kind))
            .cloned()
            .collect();
        candidates.sort_by(|a, b| a.path.cmp(&b.path));
        candidates
    }

    pub fn toggle_asset_kind(&mut self, kind: AssetKind) {
        match self.asset_query.kinds.iter().position(|item| *item == kind) {
            Some(index) => {
                self.asset_query.kinds.remove(index);
            }
            None => self.asset_query.kinds.push(kind),
        }
    }

    pub fn toggle_asset_folder(&mut self, path: &str) {
        if !self.expanded_assets.insert(path.to_string()) {
            self.expanded_assets.remove(path);
        }
    }

    // ── 主题 / 语言 ─────────────────────────────────────────────────────

    pub fn colors(&self) -> ThemeColors {
        self.theme.colors()
    }

    pub fn color(&self, token: ThemeToken) -> crate::theme::Rgba {
        self.colors().color(token)
    }

    pub fn set_theme_preset(&mut self, id: &str) {
        self.theme.set_preset(id);
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.i18n.set_locale(locale);
    }

    pub fn t(&self, key: &str) -> String {
        self.i18n.t1(key)
    }

    // ── 日志 / Play ─────────────────────────────────────────────────────

    pub fn log(&mut self, source: LogSource, level: LogLevel, text: impl Into<String>) {
        self.log.push(source, level, text);
    }

    pub fn play(&mut self) {
        self.playing = true;
        self.paused = false;
        self.play_time_s = 0.0;
        self.log(LogSource::Editor, LogLevel::Info, "进入 Play 模式");
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.paused = false;
        self.play_time_s = 0.0;
        self.log(LogSource::Editor, LogLevel::Info, "停止 Play");
    }

    pub fn pause(&mut self) {
        if self.playing {
            self.paused = true;
        }
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn step(&mut self) {
        if self.playing {
            self.paused = true;
            self.play_time_s += 1.0 / 60.0;
        }
    }

    /// 每帧推进（暂停时不推进模拟）。
    pub fn tick(&mut self, dt: f32) {
        if self.playing && !self.paused {
            self.play_time_s += dt;
        }
    }

    pub fn play_state_label(&self) -> &'static str {
        if !self.playing {
            "编辑"
        } else if self.paused {
            "暂停"
        } else {
            "运行"
        }
    }

    // ── 视口取景 ────────────────────────────────────────────────────────

    pub fn frame_all(&mut self) {
        let doc = &self.document().doc;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for entity in &doc.entities {
            any = true;
            for axis in 0..3 {
                min[axis] = min[axis].min(entity.transform.position[axis]);
                max[axis] = max[axis].max(entity.transform.position[axis]);
            }
        }
        if !any {
            return;
        }
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let mut radius: f32 = 1.0;
        for entity in &doc.entities {
            let delta = [
                entity.transform.position[0] - center[0],
                entity.transform.position[1] - center[1],
                entity.transform.position[2] - center[2],
            ];
            radius = radius
                .max((delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt());
        }
        self.camera.focus(center, radius);
    }

    pub fn frame_selection(&mut self) {
        let doc = &self.document().doc;
        let selected: Vec<&EntityJson> = self
            .selection
            .iter()
            .filter_map(|index| doc.entities.get(*index))
            .collect();
        if selected.is_empty() {
            return;
        }
        let count = selected.len() as f32;
        let center = [
            selected
                .iter()
                .map(|e| e.transform.position[0])
                .sum::<f32>()
                / count,
            selected
                .iter()
                .map(|e| e.transform.position[1])
                .sum::<f32>()
                / count,
            selected
                .iter()
                .map(|e| e.transform.position[2])
                .sum::<f32>()
                / count,
        ];
        self.camera.focus(center, 2.0);
    }

    // ── 命令分发（菜单 / 快捷键共用） ──────────────────────────────────

    pub fn run(&mut self, command: Command) {
        match command {
            Command::NewScene => self.new_document(DocKind::Scene, None),
            Command::NewPrefab => self.new_document(DocKind::Prefab, None),
            Command::OpenDocument => self.toggle_panel("docs"),
            Command::Save => match self.save_active() {
                Ok(path) => self.status = format!("已保存 {path}"),
                Err(error) => self.log(
                    LogSource::Editor,
                    LogLevel::Error,
                    format!("保存失败：{error}"),
                ),
            },
            Command::ReloadDocument => {
                if let Err(error) = self.reload_active() {
                    self.log(
                        LogSource::Editor,
                        LogLevel::Error,
                        format!("重新读取失败：{error}"),
                    );
                }
            }
            Command::RenameDocument | Command::DeleteDocument => {
                self.log(
                    LogSource::Editor,
                    LogLevel::Warn,
                    "重命名 / 删除请在资产面板或文件系统里做（GPUI 壳暂不提供破坏性操作）",
                );
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::DuplicateEntity => self.duplicate_selection(),
            Command::DeleteSelection => self.delete_selection(),
            Command::SelectAll => {
                self.selection = (0..self.document().doc.entities.len()).collect();
            }
            Command::PasteComponent => self.paste_component(),
            Command::CopyEntity => {
                self.copy_selection();
            }
            Command::CutEntity => {
                self.cut_selection();
            }
            Command::PasteEntity => {
                self.paste_entities(None);
            }
            Command::BuildProject => {
                self.log(LogSource::System, LogLevel::Info, "构建项目（占位）");
            }
            Command::CookScenes => {
                self.log(LogSource::System, LogLevel::Info, "构建场景包（占位）");
            }
            Command::Play => self.play(),
            Command::Pause => {
                if self.paused {
                    self.resume();
                } else {
                    self.pause();
                }
            }
            Command::Stop => self.stop(),
            Command::Step => self.step(),
            Command::FrameAll => self.frame_all(),
            Command::FrameSelection => self.frame_selection(),
            other => {
                if let Some(panel) = other.panel_id() {
                    self.toggle_panel(panel);
                }
            }
        }
    }

    /// 菜单里显示一项是否可用。
    pub fn is_enabled(&self, command: Command) -> bool {
        match command {
            Command::Undo => self.can_undo(),
            Command::Redo => self.can_redo(),
            Command::DuplicateEntity
            | Command::DeleteSelection
            | Command::CopyEntity
            | Command::CutEntity
            | Command::FrameSelection => !self.selection.is_empty(),
            Command::PasteEntity => self.entity_clipboard.is_some(),
            Command::PasteComponent => self.clipboard.is_some() && !self.selection.is_empty(),
            Command::Pause | Command::Stop | Command::Step => self.playing,
            Command::Play => !self.playing,
            Command::BuildProject | Command::CookScenes | Command::OfflineTools => true,
            _ => true,
        }
    }

    /// 只统计停靠在树里的 tab（不含浮动窗口）。
    pub fn docked_tabs(&self) -> Vec<String> {
        collect_tabs(&self.dock)
    }

    pub fn dock_tabs(&self) -> Vec<String> {
        let mut tabs = collect_tabs(&self.dock);
        tabs.extend(self.floating.iter().map(|panel| panel.tab.clone()));
        tabs
    }

    /// 已停靠面板的 leaf id（拖拽命中测试用）。
    pub fn leaf_of(&self, tab: &str) -> Option<String> {
        find_leaf(&self.dock, tab).map(|leaf| leaf.id().to_string())
    }

    pub fn leaf_by_id(&self, id: &str) -> Option<&DockNode> {
        find_leaf_by_id(&self.dock, id)
    }

    pub fn component_value(&self, entity: usize, component: &str) -> Option<&Value> {
        self.document()
            .doc
            .entities
            .get(entity)?
            .components
            .get(component)
    }

    pub fn prefab_links(&self) -> Vec<(usize, String)> {
        self.document().doc.prefab_links()
    }

    pub fn prefab_component_name() -> &'static str {
        PREFAB_LINK_COMPONENT
    }
}

fn hydrate_doc(schema: &Schema, doc: &mut SceneDoc) {
    for entity in &mut doc.entities {
        for (component_name, value) in entity.components.iter_mut() {
            let Some(component) = schema.component(component_name) else {
                continue;
            };
            let defaults = component.default_value();
            merge_missing(value, &defaults);
        }
    }
}

fn merge_missing(target: &mut Value, defaults: &Value) {
    if let (Value::Object(target_map), Value::Object(default_map)) = (target, defaults) {
        for (key, value) in default_map {
            match target_map.get_mut(key) {
                Some(existing) => merge_missing(existing, value),
                None => {
                    target_map.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn build_entity(
    schema: &Schema,
    preset: Option<&EntityPreset>,
    parent: Option<usize>,
    doc: &SceneDoc,
) -> EntityJson {
    let (name, components, position, rotation) = match preset {
        Some(preset) => (
            preset.name.clone(),
            preset.components.clone(),
            preset.position,
            preset.rotation,
        ),
        None => (
            "GameObject".to_string(),
            Vec::new(),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
        ),
    };
    let mut entity = EntityJson {
        name: Some(name),
        parent: parent.and_then(|index| doc.id_of_index(index)),
        transform: TransformJson {
            position,
            rotation,
            scale: [1.0; 3],
        },
        ..Default::default()
    };
    for component_name in components {
        let Some(component) = schema.component(&component_name) else {
            continue;
        };
        let override_value = preset.and_then(|preset| preset.overrides.get(&component_name));
        let value = component_value_with_overrides(&component.default_value(), override_value);
        entity.components.insert(component_name, value);
    }
    entity
}

fn default_panels() -> Vec<PanelInfo> {
    let mut panels: Vec<PanelInfo> = [
        ("sceneView", "场景视图"),
        ("gameView", "游戏视图"),
        ("docs", "文档"),
        ("hierarchy", "层级"),
        ("assets", "资产"),
        ("inspector", "检查器"),
        ("log", "日志"),
        ("renderGraph", "渲染图"),
        ("projectSettings", "项目设置"),
        ("preferences", "首选项"),
        ("theme", "界面主题"),
        ("build", "构建项目"),
        ("tools", "离线工具"),
        ("shortcuts", "键盘快捷键"),
        ("about", "关于 PrismaRev Editor"),
    ]
    .iter()
    .map(|(id, title)| PanelInfo::new(id, title))
    .collect();
    panels.sort_by(|a, b| a.id.cmp(&b.id));
    panels
}

pub fn layout_preset_ids() -> Vec<(&'static str, &'static str)> {
    LAYOUT_PRESETS
        .iter()
        .map(|preset| (preset.id, preset.label))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "schema": { "types": [
                {
                    "name": "prism_engine::scene::Camera",
                    "typeId": "c",
                    "version": 1,
                    "isComponent": true,
                    "defaultJson": { "fov_y_degrees": 60.0 },
                    "fields": [
                        { "name": "fov_y_degrees", "kind": "f32", "structType": null, "inner": null,
                          "arrayLen": null, "variants": null, "readOnly": false }
                    ]
                }
            ]},
            "presets": [
                { "id": "camera", "label": "相机", "name": "Camera",
                  "components": ["prism_engine::scene::Camera"],
                  "overrides": {}, "position": [0.0, 0.0, 10.0], "rotation": [0.0, 0.0, 0.0, 1.0] },
                { "id": "empty", "label": "空实体", "name": "entity",
                  "overrides": {}, "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0] }
            ],
            "assetInfos": [
                { "path": "assets/scenes/a.scene.json", "sizeBytes": 10, "kind": "scene", "importer": "TextScriptImporter" }
            ],
            "docs": {
                "assets/scenes/a.scene.json": {
                    "version": 3,
                    "entities": [
                        { "id": 1, "name": "Camera", "parent": null,
                          "transform": {"position":[0,1,-10],"rotation":[0,0,0,1],"scale":[1,1,1]},
                          "components": { "prism_engine::scene::Camera": { "fov_y_degrees": 60.0 } } },
                        { "id": 2, "name": "Cube", "parent": 1,
                          "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                          "components": {} }
                    ]
                }
            },
            "templateDocs": {
                "prefab": { "camera": { "version": 3, "entities": [
                    { "id": 1, "name": "Camera", "parent": null,
                      "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                      "components": {} }
                ]}}
            },
            "templatesByKind": { "prefab": [ { "id": "camera", "label": "相机" } ] },
            "windows": [
                { "id": "sceneView", "title": "场景视图", "menuPath": "窗口/场景视图", "shortcut": null }
            ]
        })
    }

    #[test]
    fn loads_fixture_documents_templates_and_panels() {
        let workspace = Workspace::load_fixture(&fixture());
        assert_eq!(workspace.docs.len(), 1);
        assert_eq!(workspace.docs[0].title(), "a.scene.json");
        assert_eq!(workspace.docs[0].doc.entities.len(), 2);
        assert_eq!(workspace.templates.len(), 1);
        assert_eq!(workspace.presets.len(), 2);
        assert_eq!(workspace.assets.len(), 1);
        assert!(workspace.panel("sceneView").is_some());
        assert_eq!(workspace.panel_title("sceneView"), "场景视图");
    }

    #[test]
    fn add_entity_uses_preset_and_parent() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.select(0, false);
        workspace.add_entity("camera");
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3);
        let added = doc.entities.last().unwrap();
        assert_eq!(added.display_name(), "Camera");
        assert_eq!(added.parent, Some(1), "挂在选中的相机下面");
        assert!(added.has_component("prism_engine::scene::Camera"));
        assert!(workspace.can_undo());
    }

    #[test]
    fn multi_field_edit_writes_all_selected_in_one_step() {
        let mut workspace = Workspace::load_fixture(&fixture());
        // 给两个实体都加 Camera 组件。
        workspace.select(0, false);
        workspace.add_component("prism_engine::scene::Camera");
        workspace.select(1, false);
        workspace.add_component("prism_engine::scene::Camera");

        let ids: Vec<u32> = workspace
            .document()
            .doc
            .entities
            .iter()
            .filter_map(|entity| entity.id)
            .collect();
        let applied = workspace.set_field_multi(
            &ids,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(55.0),
        );
        assert_eq!(applied, 2);
        for entity in &workspace.document().doc.entities {
            assert_eq!(
                entity.components["prism_engine::scene::Camera"]["fov_y_degrees"],
                json!(55.0)
            );
        }
        // 一次撤销回退全部。
        workspace.undo();
        for entity in &workspace.document().doc.entities {
            assert_eq!(
                entity.components["prism_engine::scene::Camera"]["fov_y_degrees"],
                json!(60.0)
            );
        }

        let common = workspace.common_components(&[0, 1]);
        assert!(common
            .iter()
            .any(|component| component.name == "prism_engine::scene::Camera"));
        workspace.remove_component_multi(&[0, 1], "prism_engine::scene::Camera");
        for entity in &workspace.document().doc.entities {
            assert!(!entity.has_component("prism_engine::scene::Camera") || entity.id != Some(1));
        }
    }

    #[test]
    fn rotate_and_scale_gizmo_operations_are_undoable() {
        let mut workspace = Workspace::load_fixture(&fixture());
        let id = workspace.document().doc.entities[0].id.unwrap();
        workspace.rotate_entity(id, 1, std::f32::consts::FRAC_PI_2);
        let rotation = workspace.document().doc.entities[0].transform.rotation;
        // 绕 Y 90°：w = cos45。
        assert!((rotation[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        assert!(workspace.can_undo());
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].transform.rotation,
            [0.0, 0.0, 0.0, 1.0]
        );

        workspace.set_scale_axis(id, 0, 2.5);
        assert_eq!(workspace.document().doc.entities[0].transform.scale[0], 2.5);
        workspace.set_scale_axis(id, 0, -5.0);
        assert_eq!(
            workspace.document().doc.entities[0].transform.scale[0],
            0.01,
            "负数被夹到最小值"
        );
        // 无效 delta 不产生撤销步。
        let history = workspace.document().undo.history().len();
        workspace.rotate_entity(id, 0, 0.0);
        assert_eq!(workspace.document().undo.history().len(), history);
    }

    #[test]
    fn local_gizmo_rotation_uses_entity_axis() {
        use crate::viewport_math::{basis_from_quat, quat_axis_angle, world_rotation};

        let mut workspace = Workspace::sample();
        let index = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("sample 里有 Cube");
        let id = workspace.document().doc.entities[index].id.unwrap();
        let base_rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);

        workspace.document_mut().doc.entities[index]
            .transform
            .rotation = base_rotation;
        workspace.gizmo_space = GizmoSpace::World;
        workspace.rotate_entity(id, 0, std::f32::consts::FRAC_PI_2);
        let world_x = basis_from_quat(world_rotation(&workspace.document().doc, index))[0];
        assert!(
            (world_x[1] - 1.0).abs() < 1e-3,
            "世界轴绕 X 旋后 X 轴应指向 +Y，实际 {world_x:?}"
        );

        workspace.document_mut().doc.entities[index]
            .transform
            .rotation = base_rotation;
        workspace.gizmo_space = GizmoSpace::Local;
        workspace.rotate_entity(id, 0, std::f32::consts::FRAC_PI_2);
        let local_x = basis_from_quat(world_rotation(&workspace.document().doc, index))[0];
        assert!(
            (local_x[2] + 1.0).abs() < 1e-3,
            "局部轴绕 X 旋后 X 轴应指向 −Z，实际 {local_x:?}"
        );
    }

    #[test]
    fn gizmo_basis_follows_parent_rotation_and_space() {
        let mut workspace = Workspace::sample();
        let cube = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("sample 里有 Cube");
        let detail = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Detail")
            .expect("sample 里有 Detail");
        workspace.document_mut().doc.entities[cube]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        workspace.select(detail, false);

        assert_eq!(
            workspace.gizmo_basis(),
            crate::viewport_math::IDENTITY_BASIS,
            "默认是世界空间"
        );
        workspace.toggle_gizmo_space();
        let basis = workspace.gizmo_basis();
        assert!(
            (basis[0][2] + 1.0).abs() < 1e-3,
            "父级绕 Y 90° 后子级局部 X 指向 −Z，实际 {:?}",
            basis[0]
        );
    }

    #[test]
    fn translate_entity_world_converts_into_parent_space() {
        let mut workspace = Workspace::sample();
        let cube = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("sample 里有 Cube");
        let detail = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Detail")
            .expect("sample 里有 Detail");
        workspace.document_mut().doc.entities[cube]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let id = workspace.document().doc.entities[detail].id.unwrap();
        let delta = [0.25, 0.5, -0.75];

        workspace.translate_entity_world(id, delta);

        let parent_rotation = crate::viewport_math::world_rotation(&workspace.document().doc, cube);
        let position = workspace.document().doc.entities[detail].transform.position;
        let world_offset = crate::viewport_math::quat_rotate(parent_rotation, position);
        for axis in 0..3 {
            assert!(
                (world_offset[axis] - delta[axis]).abs() < 1e-3,
                "世界位移应等于 {delta:?}，实际 {world_offset:?}"
            );
        }
        assert!(workspace.can_undo());
    }

    #[test]
    fn batch_translate_moves_all_entities_in_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.position;
        let light_before = workspace.document().doc.entities[light].transform.position;

        workspace.translate_entities_world(&[cube_id, light_id], [1.0, 0.0, 0.0]);
        workspace.translate_entities_world(&[cube_id, light_id], [1.0, 0.0, 0.0]);

        assert!(
            (workspace.document().doc.entities[cube].transform.position[0] - cube_before[0] - 2.0)
                .abs()
                < 1e-4
        );
        assert!(
            (workspace.document().doc.entities[light].transform.position[0]
                - light_before[0]
                - 2.0)
                .abs()
                < 1e-4
        );
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[cube].transform.position,
            cube_before
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.position,
            light_before
        );
        assert!(!workspace.can_undo());
    }

    #[test]
    fn batch_rotate_turns_every_selection_in_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.rotation;
        let light_before = workspace.document().doc.entities[light].transform.rotation;

        workspace.rotate_entities(&[cube_id, light_id], 1, 0.5);
        workspace.rotate_entities(&[cube_id, light_id], 1, 0.5);

        let cube_after = workspace.document().doc.entities[cube].transform.rotation;
        let light_after = workspace.document().doc.entities[light].transform.rotation;
        assert_ne!(cube_after, cube_before);
        assert_ne!(light_after, light_before);
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        let cube_restored = workspace.document().doc.entities[cube].transform.rotation;
        let light_restored = workspace.document().doc.entities[light].transform.rotation;
        for (restored, before) in [(cube_restored, cube_before), (light_restored, light_before)] {
            for (a, b) in restored.iter().zip(before) {
                assert!((a - b).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn batch_scale_uses_each_start_value_and_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.scale;
        let light_before = workspace.document().doc.entities[light].transform.scale;

        let targets = [(cube_id, cube_before[0]), (light_id, light_before[0])];
        workspace.scale_entities_axis(&targets, 0, 2.0);
        workspace.scale_entities_axis(&targets, 0, 3.0);

        assert!(
            (workspace.document().doc.entities[cube].transform.scale[0] - cube_before[0] * 3.0)
                .abs()
                < 1e-4
        );
        assert!(
            (workspace.document().doc.entities[light].transform.scale[0] - light_before[0] * 3.0)
                .abs()
                < 1e-4
        );
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[cube].transform.scale,
            cube_before
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.scale,
            light_before
        );
    }

    #[test]
    fn preview_rename_creates_one_undo_step_on_finish() {
        let mut workspace = Workspace::load_fixture(&fixture());
        let base = workspace.document().doc.clone();
        workspace.preview_rename(0, "C");
        workspace.preview_rename(0, "Ca");
        workspace.preview_rename(0, "Camera2");
        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera2"
        );
        assert!(!workspace.can_undo(), "预览阶段不产生撤销步");

        workspace.finish_rename(base);
        assert_eq!(workspace.document().undo.history().len(), 1);
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera"
        );
    }

    #[test]
    fn duplicate_selection_copies_subtree() {
        let mut workspace = Workspace::load_fixture(&fixture());
        // fixture: Camera(1) → Cube(2)。
        workspace.select(0, false);
        workspace.duplicate_selection();
        assert_eq!(workspace.document().doc.entities.len(), 4);
        let root = workspace.selection[0];
        assert_eq!(
            workspace.document().doc.entities[root].display_name(),
            "Camera (1)"
        );
        let root_id = workspace.document().doc.entities[root].id.unwrap();
        assert!(workspace
            .document()
            .doc
            .entities
            .iter()
            .any(|entity| entity.parent == Some(root_id) && entity.display_name() == "Cube"));
        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), 2);
    }

    #[test]
    fn selection_range_and_toggle_follow_anchor() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.select_range(1, 0);
        assert_eq!(workspace.selection, vec![0, 1], "反向区间也规范化");
        workspace.select(0, true);
        assert_eq!(workspace.selection, vec![1], "已有项被切换掉");
        workspace.select(0, true);
        assert_eq!(workspace.selection, vec![0, 1]);
    }

    #[test]
    fn select_many_overwrites_or_extends() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.select_many(&[1, 0], false);
        assert_eq!(workspace.selection, vec![0, 1], "排序且覆盖旧选择");
        workspace.select_many(&[1, 2], true);
        assert_eq!(workspace.selection, vec![0, 1, 2], "附加选择去重");
        workspace.select_many(&[], false);
        assert!(workspace.selection.is_empty());
    }

    #[test]
    fn selection_duplicate_delete_roundtrip() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.select(1, false);
        workspace.duplicate_selection();
        assert_eq!(workspace.document().doc.entities.len(), 3);
        assert_eq!(workspace.selection, vec![2]);

        workspace.delete_selection();
        assert_eq!(workspace.document().doc.entities.len(), 2);
        assert!(workspace.selection.is_empty());
    }

    #[test]
    fn field_edit_goes_through_undo() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(75.0),
        );
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(75.0)
        );
        assert!(workspace.document().dirty);
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );
    }

    #[test]
    fn add_and_remove_component() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.select(0, false);
        // 相机已有 Camera，列表里不应再出现。
        let available: Vec<&str> = workspace
            .component_types()
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert!(!available.contains(&"prism_engine::scene::Camera"));

        workspace.select(1, false);
        assert!(workspace
            .component_types()
            .iter()
            .any(|component| component.name == "prism_engine::scene::Camera"));
        workspace.add_component("prism_engine::scene::Camera");
        assert!(workspace.document().doc.entities[1].has_component("prism_engine::scene::Camera"));
        workspace.remove_component("prism_engine::scene::Camera");
        assert!(!workspace.document().doc.entities[1].has_component("prism_engine::scene::Camera"));
    }

    #[test]
    fn panel_toggle_and_dock_operations_keep_single_instance() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.toggle_panel("log");
        assert_eq!(
            workspace
                .dock_tabs()
                .iter()
                .filter(|tab| *tab == "log")
                .count(),
            1
        );
        workspace.toggle_panel("log");
        assert_eq!(
            workspace
                .dock_tabs()
                .iter()
                .filter(|tab| *tab == "log")
                .count(),
            1
        );

        // 浮动后从 dock 摘掉，重新停靠再回来。
        workspace.float_panel("log", 100.0, 120.0);
        assert_eq!(workspace.floating.len(), 1);
        assert!(!workspace.docked_tabs().iter().any(|tab| *tab == "log"));
        let inspector_leaf = workspace.leaf_of("inspector").unwrap();
        workspace.dock_tab("log", &inspector_leaf, DockZone::Center);
        assert!(workspace.docked_tabs().iter().any(|tab| *tab == "log"));
        assert!(workspace.floating.is_empty());
    }

    #[test]
    fn layout_presets_switch_and_clear_floating() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.float_panel("log", 10.0, 10.0);
        workspace.set_layout_preset("2by3");
        assert!(workspace.floating.is_empty());
        assert!(workspace.dock_tabs().iter().any(|tab| tab == "gameView"));
        assert_eq!(workspace.layout_preset, "2by3");
    }

    #[test]
    fn grid_toggle_flips_flag() {
        let mut workspace = Workspace::sample();
        assert!(workspace.show_grid);
        workspace.toggle_grid();
        assert!(!workspace.show_grid);
        workspace.toggle_grid();
        assert!(workspace.show_grid);
    }

    #[test]
    fn play_state_machine() {
        let mut workspace = Workspace::load_fixture(&fixture());
        assert!(!workspace.playing);
        workspace.play();
        assert!(workspace.playing);
        workspace.tick(0.5);
        assert!((workspace.play_time_s - 0.5).abs() < 1e-6);
        workspace.pause();
        workspace.tick(0.5);
        assert!((workspace.play_time_s - 0.5).abs() < 1e-6);
        workspace.step();
        assert!(workspace.play_time_s > 0.5);
        workspace.stop();
        assert!(!workspace.playing);
        assert_eq!(workspace.play_time_s, 0.0);
    }

    struct TempProject {
        root: std::path::PathBuf,
    }

    impl TempProject {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-gpui-ws-{}-{}",
                std::process::id(),
                tag
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("assets/scenes")).unwrap();
            std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn project_scan_assets_populates_tree_and_picker() {
        let project = TempProject::new("scan-assets");
        std::fs::create_dir_all(project.root.join("assets/meshes")).unwrap();
        std::fs::write(project.root.join("assets/meshes/cube.fbx"), b"fbx").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        assert!(workspace
            .assets
            .iter()
            .any(|asset| asset.path == "assets/meshes/cube.fbx" && asset.kind == AssetKind::Model));
        assert!(workspace
            .assets
            .iter()
            .any(|asset| asset.path == "assets/scenes/a.scene.json"
                && asset.kind == AssetKind::Scene));
        let meshes = workspace.asset_picker_candidates("mesh_path");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].path, "assets/meshes/cube.fbx");
        let expanded = workspace.expanded_assets.clone();
        assert!(workspace
            .tree
            .visible_rows(&expanded)
            .iter()
            .any(|(node, _)| node.name == "meshes"));
    }

    #[test]
    fn project_scene_manifest_is_loaded_and_saved() {
        let project = TempProject::new("manifest-load");
        std::fs::write(
            project.root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\nenabled = true\n\n\
             [[scenes]]\nname = \"level\"\npath = \"scenes/level.rscn\"\nenabled = false\n",
        )
        .unwrap();
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.scene_manifest.scenes.len(), 2);
        assert_eq!(workspace.scene_manifest.active().unwrap().name, "intro");

        workspace.set_scene_enabled(0, false);
        let reloaded = SceneManifest::read(&project.root).unwrap();
        assert!(!reloaded.scenes[0].enabled);
        assert!(reloaded.active().is_none(), "level 本来就没启用");

        workspace.set_scene_enabled(1, true);
        let reloaded = SceneManifest::read(&project.root).unwrap();
        assert_eq!(reloaded.active().unwrap().name, "level");
    }

    #[test]
    fn manifest_add_move_remove_and_available_documents() {
        let project = TempProject::new("manifest-edit");
        let scene = r#"{"version":3,"entities":[]}"#;
        std::fs::write(project.root.join("assets/scenes/a.scene.json"), scene).unwrap();
        std::fs::write(project.root.join("assets/scenes/b.scene.json"), scene).unwrap();
        std::fs::write(
            project.root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"a\"\npath = \"scenes/a.rscn\"\n",
        )
        .unwrap();
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();

        assert_eq!(
            workspace.available_scene_documents(),
            vec!["scenes/b.scene.json".to_string()]
        );
        let index = workspace
            .add_scene_document("assets/scenes/b.scene.json")
            .expect("b 应能加入");
        assert_eq!(index, 1);
        assert!(workspace.available_scene_documents().is_empty());
        assert!(workspace
            .add_scene_document("scenes/b.scene.json")
            .is_none());

        assert_eq!(workspace.move_scene_entry(1, -1), Some(0));
        assert_eq!(
            SceneManifest::read(&project.root).unwrap().scenes[0].name,
            "b"
        );
        assert!(workspace.remove_scene_entry(0));
        assert_eq!(SceneManifest::read(&project.root).unwrap().scenes.len(), 1);
        assert!(!workspace.remove_scene_entry(9));
    }

    #[test]
    fn delete_document_cleans_file_manifest_and_open_tab() {
        let project = TempProject::new("doc-delete");
        let scene = r#"{"version":3,"entities":[]}"#;
        std::fs::write(project.root.join("assets/scenes/a.scene.json"), scene).unwrap();
        std::fs::write(project.root.join("assets/scenes/b.scene.json"), scene).unwrap();
        std::fs::write(
            project.root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"a\"\npath = \"scenes/a.rscn\"\n\n\
             [[scenes]]\nname = \"b\"\npath = \"scenes/b.rscn\"\n",
        )
        .unwrap();
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.docs.len(), 2);
        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/a.scene.json")
        );

        workspace
            .delete_document("assets/scenes/a.scene.json")
            .unwrap();
        assert!(!project.root.join("assets/scenes/a.scene.json").exists());
        assert_eq!(workspace.docs.len(), 1);
        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/b.scene.json")
        );
        assert_eq!(workspace.project_files.len(), 1);
        let manifest = SceneManifest::read(&project.root).unwrap();
        assert_eq!(manifest.scenes.len(), 1);
        assert_eq!(manifest.scenes[0].name, "b");

        assert!(workspace
            .delete_document("assets/scenes/a.scene.json")
            .is_err());
        assert!(workspace.delete_document("assets/settings.toml").is_err());
    }

    #[test]
    fn project_roundtrip_save_and_reload() {
        let project = TempProject::new("roundtrip");
        let disk_doc = r#"{
            "version": 3,
            "entities": [
                {"id": 1, "name": "Disk Cube", "parent": null,
                 "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components": {"prism_engine::scene::Camera": {}}}
            ]
        }"#;
        std::fs::write(project.root.join("assets/scenes/disk.scene.json"), disk_doc).unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        let opened = workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(opened, 1);
        assert_eq!(workspace.project_files.len(), 1);
        assert_eq!(workspace.document().title(), "disk.scene.json");
        // 打开时补齐 schema 默认值。
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );

        // 改字段 → 保存 → dirty 清掉、文件落盘。
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(42.0),
        );
        assert!(workspace.document().dirty);
        let path = workspace.save_active().unwrap();
        assert_eq!(path, "assets/scenes/disk.scene.json");
        assert!(!workspace.document().dirty);

        // 再改（不保存）→ reload 丢弃修改。
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(90.0),
        );
        workspace.reload_active().unwrap();
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(42.0)
        );

        // 另存为到新路径，并出现在项目文件列表里。
        let saved = workspace
            .save_active_as("assets/scenes/copy.scene.json")
            .unwrap();
        assert_eq!(saved, "assets/scenes/copy.scene.json");
        assert!(project.root.join(&saved).exists());
        assert!(workspace
            .project_files
            .iter()
            .any(|file| file.path == saved));
        assert!(workspace.save_active_as("   ").is_err());
        assert!(workspace.save_active_as("outside.scene.json").is_ok());
        assert!(project
            .root
            .join("assets/scenes/outside.scene.json")
            .exists());
    }

    #[test]
    fn instantiate_prefab_asset_from_disk_and_template() {
        let project = TempProject::new("instantiate");
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Root","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        workspace.select(0, false);

        let inserted = workspace
            .instantiate_prefab_asset("assets/prefabs/box.prefab.json", Some(0))
            .unwrap();
        assert_eq!(inserted, 2);
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3);
        let instance = &doc.entities[1];
        assert_eq!(instance.parent, Some(1), "挂到选中实体下");
        assert_eq!(
            instance.prefab_link(),
            Some("assets/prefabs/box.prefab.json")
        );
        assert_eq!(workspace.selection, vec![1]);
        assert!(workspace.can_undo());

        // fixture 模式回退模板。
        let mut fixture_workspace = Workspace::load_fixture(&fixture());
        let count = fixture_workspace
            .instantiate_prefab_asset("assets/prefabs/camera.prefab.json", None)
            .unwrap();
        assert_eq!(count, 1);
        assert!(fixture_workspace
            .document()
            .doc
            .entities
            .iter()
            .any(|entity| entity.name.as_deref() == Some("Camera (1)")));

        // 不存在的预制体报错。
        assert!(workspace
            .instantiate_prefab_asset("assets/prefabs/nope.prefab.json", None)
            .is_err());
    }

    #[test]
    fn save_selection_as_prefab_writes_subtree_and_dedupes_names() {
        let project = TempProject::new("prefab-save");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Root","parent":null,
                 "transform":{"position":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        workspace.select(0, false);

        let path = workspace.save_selection_as_prefab("My Prefab").unwrap();
        assert_eq!(path, "assets/prefabs/My Prefab.prefab.json");
        let written = project::read_document(&project.root, &path).unwrap();
        assert_eq!(written.entities.len(), 2, "包含子树");
        assert!(written.entities[0].parent.is_none(), "根无父级");

        let second = workspace.save_selection_as_prefab("My Prefab").unwrap();
        assert_eq!(second, "assets/prefabs/My Prefab-2.prefab.json");
        assert!(workspace
            .project_files
            .iter()
            .any(|file| file.path == second));

        assert!(
            workspace.save_selection_as_prefab("   ").is_ok(),
            "空名字回退 Untitled"
        );
        assert!(project
            .root
            .join("assets/prefabs/Untitled.prefab.json")
            .exists());
    }

    #[test]
    fn delete_asset_only_removes_regular_assets() {
        let project = TempProject::new("delete-asset");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/a.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();

        assert!(workspace.delete_asset("outside.txt").is_err());
        assert!(workspace
            .delete_asset("assets/textures/missing.png")
            .is_err());
        assert!(workspace
            .delete_asset("assets/scenes/main.scene.json")
            .is_err());
        workspace.delete_asset("assets/textures/a.png").unwrap();
        assert!(!project.root.join("assets/textures/a.png").exists());
    }

    #[test]
    fn rename_asset_moves_file_and_rewrites_references() {
        let project = TempProject::new("rename");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/a.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Sprite","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::SpriteRenderer":{"sprite_path":"assets/textures/a.png"}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        assert!(
            workspace
                .document()
                .doc
                .field_references("assets/textures/a.png")
                .len()
                == 1
        );

        let (references, affected) = workspace
            .rename_asset("assets/textures/a.png", "assets/textures/b.png")
            .unwrap();
        assert_eq!(references, 1);
        assert_eq!(affected.len(), 1);
        assert!(!project.root.join("assets/textures/a.png").exists());
        assert!(project.root.join("assets/textures/b.png").exists());
        assert_eq!(
            workspace
                .document()
                .doc
                .field_references("assets/textures/b.png")
                .len(),
            1
        );
        assert!(workspace.document().dirty, "引用改写后应标记未保存");

        // 场景文档不允许走资产重命名。
        assert!(workspace
            .rename_asset(
                "assets/scenes/main.scene.json",
                "assets/scenes/other.scene.json"
            )
            .is_err());
        // 目标必须存在 / 在 assets 内。
        assert!(workspace
            .rename_asset("assets/textures/b.png", "outside.png")
            .is_err());
    }

    #[test]
    fn revert_prefab_instance_from_disk_keeps_name_and_parent() {
        let project = TempProject::new("revert");
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[5,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::Camera":{"fov_y_degrees":33.0}}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Parent","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"My Box","parent":1,
                 "transform":{"position":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::PrefabInstance":{"prefab":"assets/prefabs/box.prefab.json"}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.document().doc.entities.len(), 2);

        workspace.revert_prefab_instance(2).unwrap();
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3, "实例根 + 预制体子级");
        let root = doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("My Box"))
            .expect("保留实例名字");
        assert_eq!(root.parent, Some(1), "保留父级");
        assert_eq!(
            root.transform.position,
            [5.0, 0.0, 0.0],
            "使用预制体当前内容"
        );
        assert_eq!(
            root.prefab_link(),
            Some("assets/prefabs/box.prefab.json"),
            "链接保留"
        );
        assert!(doc
            .entities
            .iter()
            .any(|entity| entity.display_name() == "Child"));

        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), 2);
        assert_eq!(
            workspace.document().doc.entities[1].transform.position,
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn prefab_falls_back_to_template_in_fixture_mode() {
        let mut workspace = Workspace::load_fixture(&fixture());
        // fixture 的模板是 camera；伪造一个指向它的实例。
        workspace.document_mut().doc.entities[1].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({"prefab": "assets/prefabs/camera.prefab.json"}),
        );
        let doc = workspace.prefab_document_for(2).unwrap();
        assert_eq!(doc.entities.len(), 1);
        assert_eq!(doc.entities[0].display_name(), "Camera");

        workspace.revert_prefab_instance(2).unwrap();
        let instance = workspace
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Cube"))
            .expect("实例根");
        assert_eq!(
            instance.prefab_link(),
            Some("assets/prefabs/camera.prefab.json")
        );
        // 非实例实体不能被还原。
        assert!(workspace.revert_prefab_instance(1).is_err());
    }

    #[test]
    fn saving_without_project_fails_cleanly() {
        let mut workspace = Workspace::load_fixture(&fixture());
        assert!(!workspace.has_project());
        assert!(workspace.save_active().is_err());
        assert!(workspace.reload_active().is_err());
    }

    #[test]
    fn hydration_fills_missing_component_fields() {
        let mut fixture = fixture();
        // 去掉 Camera 的 fov_y_degrees，模拟旧文档。
        fixture["docs"]["assets/scenes/a.scene.json"]["entities"][0]["components"]
            ["prism_engine::scene::Camera"] = json!({});
        let mut workspace = Workspace::load_fixture(&fixture);
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(80.0),
        );
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(80.0)
        );
    }

    #[test]
    fn unlink_prefab_removes_link_only_once() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.document_mut().doc.entities[1].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({ "prefab": "assets/prefabs/camera.prefab.json" }),
        );
        assert!(workspace.unlink_prefab(1));
        assert!(!workspace.document().doc.entities[1].has_component(PREFAB_LINK_COMPONENT));
        assert!(!workspace.unlink_prefab(1), "没有链接时返回 false");
    }

    #[test]
    fn asset_path_fields_are_detected_and_filtered() {
        use crate::assets::{is_asset_path_field, AssetKind};
        assert!(is_asset_path_field(
            "prism_engine::scene::MeshRenderer",
            "mesh_path"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::Skybox",
            "hdr_path"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::EnvironmentLighting",
            "env_map"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::PrefabInstance",
            "prefab"
        ));
        assert!(!is_asset_path_field("prism_engine::scene::Camera", "name"));
        assert!(!is_asset_path_field(
            "prism_engine::scene::Camera",
            "fov_y_degrees"
        ));

        let mut workspace = Workspace::sample();
        use crate::assets::AssetInfo;
        workspace.assets = vec![
            AssetInfo {
                path: "assets/meshes/cube.rmesh".into(),
                size_bytes: 1,
                kind: AssetKind::Model,
                importer: "ModelImporter".into(),
            },
            AssetInfo {
                path: "assets/textures/grid.png".into(),
                size_bytes: 1,
                kind: AssetKind::Texture,
                importer: "TextureImporter".into(),
            },
        ];
        let meshes = workspace.asset_picker_candidates("mesh_path");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].path, "assets/meshes/cube.rmesh");
        let textures = workspace.asset_picker_candidates("sprite_path");
        assert_eq!(textures.len(), 1);
        assert_eq!(textures[0].kind, AssetKind::Texture);
        let all = workspace.asset_picker_candidates("some_unknown_field");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn theme_and_locale_switch() {
        let mut workspace = Workspace::load_fixture(&fixture());
        assert_eq!(workspace.t("common.save"), "保存");
        workspace.set_locale(Locale::EnUs);
        assert_eq!(workspace.t("common.save"), "Save");
        workspace.set_theme_preset("paper");
        assert_eq!(workspace.color(ThemeToken::Bg).to_hex(), "#f2f4f7");
    }

    #[test]
    fn frame_all_moves_camera_target() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.frame_all();
        assert!(workspace.camera.distance > 0.5);
        workspace.select(0, false);
        workspace.frame_selection();
        assert_eq!(workspace.camera.target[1], 1.0);
    }

    #[test]
    fn new_document_from_template() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.new_document(DocKind::Prefab, Some("camera"));
        assert_eq!(workspace.docs.len(), 2);
        assert_eq!(workspace.document().kind, DocKind::Prefab);
        assert!(workspace
            .document()
            .path
            .as_deref()
            .unwrap()
            .ends_with("camera.prefab.json"));
    }

    #[test]
    fn commands_respect_enabled_state() {
        let mut workspace = Workspace::load_fixture(&fixture());
        assert!(!workspace.is_enabled(Command::Undo));
        workspace.run(Command::SelectAll);
        assert_eq!(workspace.selection.len(), 2);
        workspace.run(Command::DuplicateEntity);
        assert!(workspace.can_undo());
        assert!(workspace.is_enabled(Command::Undo));
        workspace.run(Command::Play);
        assert!(workspace.is_enabled(Command::Stop));
        assert!(!workspace.is_enabled(Command::Play));
    }

    #[test]
    fn undo_snapshot_roundtrip_for_reparent() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.reparent(1, None, Some(1));
        assert!(workspace.document().doc.entities[1].parent.is_none());
        workspace.undo();
        assert_eq!(workspace.document().doc.entities[1].parent, Some(1));
    }

    fn index_named(workspace: &Workspace, name: &str) -> usize {
        workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == name)
            .unwrap_or_else(|| panic!("missing entity {name}"))
    }

    #[test]
    fn selection_roots_drops_nested_entities() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let detail = index_named(&workspace, "Detail");
        workspace.select(cube, false);
        workspace.select(detail, true);
        assert_eq!(workspace.selection_roots(), vec![cube]);
    }

    #[test]
    fn copy_paste_keeps_subtree_and_attaches_parent() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.select(cube, false);
        assert_eq!(workspace.copy_selection(), 2, "Cube + Detail");

        let before = workspace.document().doc.entities.len();
        let pasted = workspace.paste_entities(None);
        assert_eq!(pasted.len(), 1);
        assert_eq!(workspace.document().doc.entities.len(), before + 2);
        assert_eq!(workspace.selection, pasted);
        let root = &workspace.document().doc.entities[pasted[0]];
        assert_eq!(root.display_name(), "Cube (1)");
        assert!(root.parent.is_none());

        let pasted = workspace.paste_entities(Some(light));
        let light_id = workspace.document().doc.entities[light].id;
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].parent,
            light_id
        );
    }

    #[test]
    fn cut_removes_source_then_paste_restores() {
        let mut workspace = Workspace::sample();
        let before = workspace.document().doc.entities.len();
        let detail = index_named(&workspace, "Detail");
        workspace.select(detail, false);
        assert_eq!(workspace.cut_selection(), 1);
        assert!(workspace
            .document()
            .doc
            .entities
            .iter()
            .all(|entity| entity.display_name() != "Detail"));
        assert!(workspace.entity_clipboard.as_ref().unwrap().cut);

        let pasted = workspace.paste_entities(None);
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].display_name(),
            "Detail"
        );
        workspace.undo();
        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), before);
        assert_eq!(
            workspace.document().doc.entities[detail].display_name(),
            "Detail"
        );
    }

    #[test]
    fn copy_preserves_prefab_instance_link() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        workspace.document_mut().doc.entities[cube]
            .components
            .insert(
                PREFAB_LINK_COMPONENT.to_string(),
                serde_json::json!({ "prefab": "assets/prefabs/cube.prefab.json" }),
            );
        workspace.select(cube, false);
        workspace.copy_selection();
        let pasted = workspace.paste_entities(None);
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].prefab_link(),
            Some("assets/prefabs/cube.prefab.json")
        );
    }

    #[test]
    fn duplicate_multiple_roots_copies_each_subtree() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.select(cube, false);
        workspace.select(light, true);
        let before = workspace.document().doc.entities.len();
        workspace.run(Command::DuplicateEntity);
        assert_eq!(workspace.document().doc.entities.len(), before + 3);
        assert_eq!(workspace.selection.len(), 2);
    }

    #[test]
    fn entity_clipboard_commands_enable_state() {
        let mut workspace = Workspace::sample();
        assert!(!workspace.is_enabled(Command::PasteEntity));
        workspace.select(0, false);
        assert!(workspace.is_enabled(Command::CopyEntity));
        workspace.run(Command::CopyEntity);
        assert!(workspace.is_enabled(Command::PasteEntity));
        workspace.run(Command::CutEntity);
        assert!(workspace.entity_clipboard.as_ref().unwrap().cut);
    }
}
