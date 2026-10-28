import type {
  AssetImportInfo,
  AssetInfo,
  AssetMutation,
  AssetRef,
  AssetTextPreview,
  BakeGiOptions,
  BuildProfile,
  BuildProjectOptions,
  BuildProjectResult,
  CameraOrbit,
  DocKind,
  EditorPaths,
  EditorWindowDescriptor,
  EntityPreset,
  HeightmapOptions,
  InstantiateResult,
  JsonValue,
  MoveResult,
  PlayableScene,
  PrefabAssetType,
  PrefabInstanceInfo,
  PrefabLink,
  PrefabUnpackMode,
  ProjectSettingsData,
  PreviewLogLine,
  RenderGraphReport,
  RenderSettingsData,
  SceneDoc,
  SceneEntry,
  SceneListEntry,
  SceneMeta,
  ScenePackageStatus,
  SceneTemplate,
  Schema,
  SelectionMode,
  ViewportInput,
  ViewportSnapshot,
} from "./types";

/**
 * Tauri 命令的类型化包装。
 *
 * 所有场景/预制体/schema 逻辑都在 Rust 侧（`prism-editor-api`）；这里只做
 * `invoke` 的名字与参数映射，前端不重复实现任何 schema 逻辑。
 */

/** 是否运行在 Tauri 宿主里。 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
/** 浏览器预览必须显式带 `?demo=1`，避免真实运行时静默回退到 mock。 */
const demoRequested =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).has("demo");

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  if (import.meta.env.DEV && demoRequested) {
    // 仅开发服务器 + 显式 ?demo=1 才加载 fixture；生产构建会 tree-shake 掉。
    const mock = await import("./mock");
    return mock.invoke<T>(cmd, args);
  }
  throw new Error(
    `Mock 已禁用：${cmd} 只能在 Tauri 宿主中执行。` +
      `（纯浏览器预览请打开 /?demo=1，数据来自只读 fixture）`,
  );
}

// ── schema / 元信息 ─────────────────────────────────────────────────────────

/** 组件属性面板 schema。 */
export function fetchSchema(): Promise<Schema> {
  return call<Schema>("editor_schema");
}

/** 编辑器面板注册表 = Unity `EditorWindow` / `[MenuItem]` 的声明式 DTO。 */
export function fetchEditorWindows(): Promise<EditorWindowDescriptor[]> {
  return call<EditorWindowDescriptor[]>("editor_windows");
}

/** Unity `Build.Profile.BuildProfile`（目标组 / 构建选项 / 场景构建顺序）。 */
export function fetchBuildProfile(): Promise<BuildProfile> {
  return call<BuildProfile>("editor_build_profile");
}

/** 项目根（含 assets/scenes）。 */
export function fetchProjectRoot(): Promise<string> {
  return call<string>("editor_project_root");
}

/** 快速添加实体的预设。 */
export function fetchPresets(): Promise<EntityPreset[]> {
  return call<EntityPreset[]>("editor_entity_presets");
}

/** `assets/` 下的候选路径（相对项目根）。 */
export function listAssets(): Promise<string[]> {
  return call<string[]>("editor_list_assets");
}

/** 资产元信息（路径 / 大小 / 分类）。 */
export function listAssetsInfo(): Promise<AssetInfo[]> {
  return call<AssetInfo[]>("editor_list_assets_info");
}

/** `assets/` 下的目录（含空目录）。 */
export function listAssetDirs(): Promise<string[]> {
  return call<string[]>("editor_list_asset_dirs");
}

/** 读取文本资产（脚本 / JSON / TOML / 着色器等）。 */
export function readAssetText(path: string, maxBytes = 512 * 1024): Promise<AssetTextPreview> {
  return call<AssetTextPreview>("editor_read_asset_text", { path, maxBytes });
}

/**
 * 读取资产原始字节（图片预览）。
 *
 * Tauri 对 `tauri::ipc::Response` 直接返回 ArrayBuffer，不经过 JSON；
 * 浏览器预览没有文件访问能力，直接报错而不是伪造数据。
 */
export async function readAssetBytes(
  path: string,
  maxBytes = 16 * 1024 * 1024,
): Promise<Uint8Array> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    const data = await invoke<ArrayBuffer | Uint8Array>("editor_read_asset_bytes", {
      path,
      maxBytes,
    });
    return data instanceof Uint8Array ? data : new Uint8Array(data);
  }
  if (import.meta.env.DEV && demoRequested) {
    throw new Error("浏览器预览模式无法读取资产二进制；请用 `pnpm tauri dev`");
  }
  throw new Error("Mock 已禁用：editor_read_asset_bytes 只能在 Tauri 宿主中执行");
}

/** 哪些文档字段引用了该资产。 */
export function assetUsage(path: string): Promise<AssetRef[]> {
  return call<AssetRef[]>("editor_asset_usage", { path });
}

/** 新建资产目录。 */
export function createAssetDir(path: string): Promise<string> {
  return call<string>("editor_create_asset_dir", { path });
}

/** 重命名 / 移动资产，并同步文档引用。 */
export function renameAsset(from: string, to: string): Promise<AssetMutation> {
  return call<AssetMutation>("editor_rename_asset", { from, to });
}

/** 跨目录移动资产。 */
export function moveAsset(from: string, to: string): Promise<AssetMutation> {
  return call<AssetMutation>("editor_move_asset", { from, to });
}

/** 删除前的影响范围（引用了该资产的文档）。 */
export function assetReferences(path: string): Promise<string[]> {
  return call<string[]>("editor_asset_references", { path });
}

/** 删除资产；仍有引用时需 `force = true`。 */
export function deleteAsset(path: string, force: boolean): Promise<AssetMutation> {
  return call<AssetMutation>("editor_delete_asset", { path, force });
}

// ── 文档（场景 / 预制体） ───────────────────────────────────────────────────

/** 枚举某类文档。 */
export function listDocuments(kind: DocKind): Promise<SceneEntry[]> {
  return call<SceneEntry[]>("editor_list_documents", { kind });
}

/** 某类文档的可用模板。 */
export function documentTemplates(kind: DocKind): Promise<SceneTemplate[]> {
  return call<SceneTemplate[]>("editor_document_templates", { kind });
}

/** 按模板新建文档。 */
export function newDocument(kind: DocKind, template: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_new_document", { kind, template });
}

/** 读取文档（已规范化）。 */
export function readDocument(path: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_read_document", { path });
}

/** 校验并写回文档。 */
export function writeDocument(path: string, doc: SceneDoc): Promise<void> {
  return call<void>("editor_write_document", { path, doc });
}

/** 另存为新名字（不覆盖），返回新路径。 */
export function writeDocumentAs(kind: DocKind, name: string, doc: SceneDoc): Promise<string> {
  return call<string>("editor_write_document_as", { kind, name, doc });
}

/** 重命名文档，返回新路径。 */
export function renameDocument(path: string, kind: DocKind, name: string): Promise<string> {
  return call<string>("editor_rename_document", { path, kind, name });
}

/** 删除文档。 */
export function deleteDocument(path: string): Promise<void> {
  return call<void>("editor_delete_document", { path });
}

/** 校验文档（不落盘），失败时抛出错误描述。 */
export function validateDocument(doc: SceneDoc): Promise<void> {
  return call<void>("editor_validate_document", { doc });
}

// ── 层级操作 ───────────────────────────────────────────────────────────────

/** 改父级（`parent = null` 表示变根）；拒绝自父与成环。 */
export function reparentEntity(
  doc: SceneDoc,
  child: number,
  parent: number | null,
): Promise<SceneDoc> {
  return call<SceneDoc>("editor_reparent_entity", { doc, child, parent });
}

/** 移动到数组的 `at` 位置并设置父级。 */
export function moveEntity(
  doc: SceneDoc,
  child: number,
  at: number,
  parent: number | null,
): Promise<MoveResult> {
  return call<MoveResult>("editor_move_entity", { doc, child, at, parent });
}

/** 批量删除实体。 */
export function deleteEntities(doc: SceneDoc, indices: number[]): Promise<SceneDoc> {
  return call<SceneDoc>("editor_delete_entities", { doc, indices });
}

// ── 预制体 ─────────────────────────────────────────────────────────────────

/** 把「某实体 + 其后代」保存为预制体，返回新文件路径。 */
export function createPrefabFromEntity(
  doc: SceneDoc,
  entityIndex: number,
  name: string,
): Promise<string> {
  return call<string>("editor_create_prefab_from_entity", { doc, entityIndex, name });
}

/** 把实体子树保存为预制体到指定资产目录（层级树拖放）。 */
export function createPrefabAt(
  doc: SceneDoc,
  entityIndex: number,
  folder: string,
  name: string,
): Promise<string> {
  return call<string>("editor_create_prefab_at", { doc, entityIndex, folder, name });
}

/** 把预制体实例化进场景，返回新文档与插入下标（同时记录反向链接）。 */
export function instantiatePrefab(
  scene: SceneDoc,
  prefabPath: string,
  parent: number | null,
): Promise<InstantiateResult> {
  return call<InstantiateResult>("editor_instantiate_prefab", { scene, prefabPath, parent });
}

/** 文档里所有预制体实例根。 */
export function prefabInstances(doc: SceneDoc): Promise<PrefabLink[]> {
  return call<PrefabLink[]>("editor_prefab_instances", { doc });
}

/** 把某个实例根还原成预制体的当前内容。 */
export function revertPrefabInstance(
  doc: SceneDoc,
  rootIndex: number,
  prefab: string,
): Promise<MoveResult> {
  return call<MoveResult>("editor_revert_prefab_instance", { doc, rootIndex, prefab });
}

/** 把文档里所有引用该预制体的实例都还原一遍。 */
export function syncPrefabInstances(doc: SceneDoc, prefab: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_sync_prefab_instances", { doc, prefab });
}

/** 解除实例链接（不动实体内容）。 */
export function unlinkPrefabInstance(doc: SceneDoc, rootIndex: number): Promise<SceneDoc> {
  return call<SceneDoc>("editor_unlink_prefab_instance", { doc, rootIndex });
}

/** 提取「实体 + 后代」为预制体文档（「应用到预制体」用）。 */
export function extractSubtree(doc: SceneDoc, rootIndex: number): Promise<SceneDoc> {
  return call<SceneDoc>("editor_extract_subtree", { doc, rootIndex });
}

// ── 反射字段写入 / cook ─────────────────────────────────────────────────────

/**
 * **基于反射**写入组件字段：只读字段会被拒绝，值经反射类型校验，
 * 返回规范化后的组件 JSON。
 */
export function setComponentField(
  component: string,
  before: JsonValue,
  path: string,
  after: JsonValue,
): Promise<JsonValue> {
  return call<JsonValue>("editor_set_component_field", { component, before, path, after });
}

/** 把场景 cook 成 RSCN，返回命令输出。 */
export function cookScene(path: string): Promise<string> {
  return call<string>("editor_cook_scene", { path });
}

// ── 运行 / 预览 ─────────────────────────────────────────────────────────────

/** `assets/scenes.toml` 中的可运行场景名。 */
export function listPlayableScenes(): Promise<PlayableScene[]> {
  return call<PlayableScene[]>("editor_list_playable_scenes");
}

/** 读取可编辑的 Build Settings 场景列表。 */
export function sceneManifest(): Promise<SceneListEntry[]> {
  return call<SceneListEntry[]>("editor_scene_manifest");
}

/** 写回 Build Settings 场景列表（顺序 / enabled）。 */
export function writeSceneManifest(scenes: SceneListEntry[]): Promise<void> {
  return call<void>("editor_write_scene_manifest", { scenes });
}

/** 运行时场景包状态。 */
export function scenePackageStatus(): Promise<ScenePackageStatus> {
  return call<ScenePackageStatus>("editor_scene_package_status");
}

/** 重新 cook + 打包所有场景（`build-scenes`），返回命令输出。 */
export function buildScenes(): Promise<string> {
  return call<string>("editor_build_scenes");
}

/** 后台构建 / 导入进度（无任务时返回当前最后一次的状态或 `null`）。 */
export function buildStatus(): Promise<{
  active: boolean;
  label: string;
  message: string;
  elapsedMs: number;
} | null> {
  return call("editor_build_status");
}

// ── 离线工具 ────────────────────────────────────────────────────────────────

/** 离线烘焙 GI probe volume，返回命令输出。 */
export function bakeGi(options: BakeGiOptions): Promise<string> {
  return call<string>("editor_bake_gi", { options });
}

/** 生成程序化高度图，返回命令输出。 */
export function generateHeightmap(options: HeightmapOptions): Promise<string> {
  return call<string>("editor_generate_heightmap", { options });
}

/** 启动游戏预览指定场景，返回进程 PID。 */
export function playScene(scene: string): Promise<number> {
  return call<number>("editor_play_scene", { scene });
}

/** 停止预览进程。 */
export function stopPreview(): Promise<boolean> {
  return call<boolean>("editor_stop_preview");
}

/** 预览进程是否仍在运行。 */
export function previewRunning(): Promise<boolean> {
  return call<boolean>("editor_preview_running");
}

/** 已捕获的游戏日志（面板重新显示时回填）。 */
export function previewLogs(): Promise<PreviewLogLine[]> {
  return call<PreviewLogLine[]>("editor_preview_logs");
}

/** 订阅游戏进程 stdout/stderr。返回取消订阅函数。 */
export async function onPreviewLog(
  handler: (entry: PreviewLogLine) => void,
): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<PreviewLogLine>("preview-log", (event) => handler(event.payload));
}

// ── 项目设置 ────────────────────────────────────────────────────────────────

/** 读取项目级渲染设置。 */
export function renderSettings(): Promise<RenderSettingsData> {
  return call<RenderSettingsData>("editor_render_settings");
}

/**
 * 写入项目级渲染设置。
 *
 * 只提交 `before`（面板打开时的快照）→ `after` 之间真正改动的字段，
 * 避免把别的面板改过的字段用旧值写回去。
 */
export function writeRenderSettings(
  before: RenderSettingsData,
  after: RenderSettingsData,
): Promise<void> {
  return call<void>("editor_write_render_settings", { before, after });
}

/** 读取项目设置（app/window/build）。 */
export function projectSettings(): Promise<ProjectSettingsData> {
  return call<ProjectSettingsData>("editor_project_settings");
}

/**
 * 写入项目设置（保留 [render]）。
 *
 * 只提交 `before`（面板打开时的快照）→ `after` 之间真正改动的字段。
 */
export function writeProjectSettings(
  before: ProjectSettingsData,
  after: ProjectSettingsData,
): Promise<void> {
  return call<void>("editor_write_project_settings", { before, after });
}

/** 构建游戏 exe 并汇总运行所需资产。 */
export function buildProject(options: BuildProjectOptions): Promise<BuildProjectResult> {
  return call<BuildProjectResult>("editor_build_project", { options });
}

/** 在资源管理器中显示文件/目录。 */
export function revealPath(path: string): Promise<void> {
  return call<void>("editor_reveal_path", { path });
}

// ── 渲染图 ──────────────────────────────────────────────────────────────────

/** 读取游戏进程写出的最新渲染图报告（未运行过预览时为 null）。 */
export function renderGraphReport(): Promise<RenderGraphReport | null> {
  return call<RenderGraphReport | null>("editor_render_graph_report");
}

// ── AssetDatabase（对应 `UnityEditor.AssetDatabase`） ───────────────────────

/** 资产路径 → GUID（路径派生，故重命名会变）。 */
export function assetGuid(path: string): Promise<string> {
  return call<string>("editor_asset_guid", { path });
}

/** GUID → 资产路径。 */
export function assetPathFromGuid(guid: string): Promise<string> {
  return call<string>("editor_asset_path_from_guid", { guid });
}

/** 全部资产路径 = `GetAllAssetPaths`。 */
export function assetAllPaths(): Promise<string[]> {
  return call<string[]>("editor_asset_all_paths");
}

/**
 * Unity 风格检索 = `FindAssets`：空白分词取 **AND**，
 * `t:<分类>` 按分类、`l:<标签>` 暂被忽略，其余按路径子串。
 */
export function findAssets(filter: string): Promise<string[]> {
  return call<string[]>("editor_find_assets", { filter });
}

/** 主资产类型 = `GetMainAssetTypeAtPath`。 */
export function assetMainType(path: string): Promise<string> {
  return call<string>("editor_asset_main_type", { path });
}

/** 依赖哈希（路径 + 大小 + mtime）= `GetAssetDependencyHash`。 */
export function assetDependencyHash(path: string): Promise<string> {
  return call<string>("editor_asset_dependency_hash", { path });
}

/** 资产是否存在 = `Contains`。 */
export function assetContains(path: string): Promise<boolean> {
  return call<boolean>("editor_asset_contains", { path });
}

/** `assets/` 下是否已有该目录 = `IsValidFolder`。 */
export function assetIsValidFolder(path: string): Promise<boolean> {
  return call<boolean>("editor_asset_is_valid_folder", { path });
}

/** 复制资产（目录递归）= `CopyAsset`；**不改写引用**。 */
export function copyAsset(from: string, to: string): Promise<AssetMutation> {
  return call<AssetMutation>("editor_copy_asset", { from, to });
}

/** 导入资产 = `ImportAsset`（存在性校验 + 分类）。 */
export function importAsset(path: string): Promise<AssetImportInfo> {
  return call<AssetImportInfo>("editor_import_asset", { path });
}

/** 刷新资产数据库 = `Refresh`；返回资产数量。 */
export function refreshAssets(): Promise<number> {
  return call<number>("editor_refresh_assets");
}

// ── PrefabUtility（对应 `UnityEditor.PrefabUtility`） ───────────────────────

/** 把实例内容写回预制体文件 = `ApplyPrefabInstance`。 */
export function applyPrefabInstance(doc: SceneDoc, rootIndex: number): Promise<SceneDoc> {
  return call<SceneDoc>("editor_apply_prefab_instance", { doc, rootIndex });
}

/** 解包实例 = `UnpackPrefabInstance`（断开链接，内容保留）。 */
export function unpackPrefabInstance(
  doc: SceneDoc,
  rootIndex: number,
  mode: PrefabUnpackMode = "outermost_root",
): Promise<SceneDoc> {
  return call<SceneDoc>("editor_unpack_prefab_instance", { doc, rootIndex, mode });
}

/** 预制体资产类型 = `GetPrefabAssetType`。 */
export function prefabAssetType(doc: SceneDoc): Promise<PrefabAssetType> {
  return call<PrefabAssetType>("editor_prefab_asset_type", { doc });
}

/** 文档里全部实例信息。 */
export function prefabInstanceInfos(doc: SceneDoc): Promise<PrefabInstanceInfo[]> {
  return call<PrefabInstanceInfo[]>("editor_prefab_instance_infos", { doc });
}

/** 实体所属的最近实例根 = `GetNearestPrefabInstanceRoot`。 */
export function nearestPrefabInstanceRoot(
  doc: SceneDoc,
  rootIndex: number,
): Promise<number | null> {
  return call<number | null>("editor_nearest_prefab_instance_root", { doc, rootIndex });
}

/** 实体是否属于某个实例 = `IsPartOfPrefabInstance`。 */
export function isPartOfPrefabInstance(doc: SceneDoc, rootIndex: number): Promise<boolean> {
  return call<boolean>("editor_is_part_of_prefab_instance", { doc, rootIndex });
}

// ── SceneManagement（对应 `UnityEditor.SceneManagement`） ───────────────────

/** 场景元信息列表（名字 / 路径 / GUID / 实体数 / 构建序号）。 */
export function scenesMeta(): Promise<SceneMeta[]> {
  return call<SceneMeta[]>("editor_scenes_meta");
}

/** `assets/scenes` 下的场景数量。 */
export function sceneMetaCount(): Promise<number> {
  return call<number>("editor_scene_meta_count");
}

/** 按下标取场景元信息。 */
export function sceneMetaAt(index: number): Promise<SceneMeta> {
  return call<SceneMeta>("editor_scene_meta_at", { index });
}

/** 按名字解析场景元信息。 */
export function resolveScene(name: string): Promise<SceneMeta> {
  return call<SceneMeta>("editor_resolve_scene", { name });
}

/** 新建场景文档 = `EditorSceneManager.NewScene`（`template` 缺省用默认模板）。 */
export function newScene(template?: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_new_scene", { template: template ?? null });
}

/** 为「尚未落盘」的文档生成元信息。 */
export function sceneFromDoc(name: string, path: string, doc: SceneDoc): Promise<SceneMeta> {
  return call<SceneMeta>("editor_scene_from_doc", { name, path, doc });
}

// ── Selection（对应 `UnityEditor.Selection.GetFiltered`） ───────────────────

/** 在给定下标集合上按模式过滤。 */
export function selectionFiltered(
  doc: SceneDoc,
  indices: number[],
  mode: SelectionMode,
  component?: string,
): Promise<number[]> {
  return call<number[]>("editor_selection_filtered", {
    doc,
    indices,
    mode,
    component: component ?? null,
  });
}

/** 按模式过滤文档里的**全部**实体（「选择所有同类型」）。 */
export function selectionAllMatching(
  doc: SceneDoc,
  mode: SelectionMode,
  component?: string,
): Promise<number[]> {
  return call<number[]>("editor_selection_all_matching", {
    doc,
    mode,
    component: component ?? null,
  });
}

// ── EditorApplication（对应 `UnityEditor.EditorApplication` / `Application`） ──

/** 引擎版本（「关于」面板用）。 */
export function engineVersion(): Promise<string> {
  return call<string>("editor_engine_version");
}

/** 编辑器关键路径（`Application.dataPath` 等）。 */
export function editorPaths(): Promise<EditorPaths> {
  return call<EditorPaths>("editor_paths");
}

// ── 内置视口（Scene / Game view，进程内 engine 实例） ──────────────────────

/**
 * 打开视口：后端起一个持有 `prism_app::App` 的线程，帧经 `channel` 以
 * `ArrayBuffer`（8 字节宽高头 + RGBA8）推回来。
 *
 * `channel` 是 `@tauri-apps/api` 导出的 `Channel`；这里用 `unknown` 避免
 * 让 api.ts 依赖 Tauri 运行时（浏览器预览时不可用）。
 */
export function viewportOpen(
  id: string,
  kind: "scene" | "game",
  doc: SceneDoc,
  width: number,
  height: number,
  channel: unknown,
  camera?: CameraOrbit,
  playing?: boolean,
): Promise<ViewportSnapshot> {
  return call<ViewportSnapshot>("editor_viewport_open", {
    id,
    kind,
    doc,
    width,
    height,
    channel,
    camera: camera ?? null,
    playing: playing ?? null,
  });
}

/** 面板尺寸变化（物理像素）。 */
export function viewportResize(id: string, width: number, height: number): Promise<void> {
  return call<void>("editor_viewport_resize", { id, width, height });
}

/** 面板不可见时暂停（停靠到后台标签）。 */
export function viewportSetActive(id: string, active: boolean): Promise<void> {
  return call<void>("editor_viewport_set_active", { id, active });
}

/** Scene view 相机（轨道参数整体下发）。 */
export function viewportCamera(id: string, camera: CameraOrbit): Promise<void> {
  return call<void>("editor_viewport_camera", { id, camera });
}

/** 用新文档重建场景（保存 / 撤销 / 改字段后 debounce 调用）。 */
export function viewportReload(id: string, doc: SceneDoc): Promise<ViewportSnapshot> {
  return call<ViewportSnapshot>("editor_viewport_reload", { id, doc });
}

/** Game view 的 Play / Stop。 */
export function viewportSetPlaying(id: string, playing: boolean): Promise<void> {
  return call<void>("editor_viewport_set_playing", { id, playing });
}

/** Ctrl+R：立即重编并重载项目模块（无视 Auto Refresh 开关）。 */
export function viewportRefreshProject(id: string): Promise<void> {
  return call<void>("editor_viewport_refresh_project", { id });
}

/** 项目模块自动编译开关（Unity 的 Auto Refresh）。 */
export function viewportSetAutoRefresh(enabled: boolean): Promise<void> {
  return call<void>("editor_viewport_set_auto_refresh", { enabled });
}

export function viewportAutoRefresh(): Promise<boolean> {
  return call<boolean>("editor_viewport_auto_refresh");
}

/** Game view 的鼠标 / 键盘事件。 */
export function viewportInput(id: string, event: ViewportInput): Promise<void> {
  return call<void>("editor_viewport_input", { id, event });
}

/** 轮询视口状态（FPS / 实体数 / 加载错误 / 实际渲染尺寸）。 */
export function viewportStats(id: string): Promise<ViewportSnapshot | null> {
  return call<ViewportSnapshot | null>("editor_viewport_stats", { id });
}

/** 高亮选中项（`index` = 文档 `entities` 下标；`null` = 取消选中）。 */
export function viewportSelect(id: string, index: number | null): Promise<void> {
  return call<void>("editor_viewport_select", { id, index });
}

/** 拖动过程中的实时交互反馈（浮层数值提示、旋转弧面等）。 */
export interface DragFeedback {
  tool: "translate" | "rotate" | "scale";
  axis: string;
  text: string;
  centerScreen: [number, number];
  startScreen?: [number, number];
  currentScreen?: [number, number];
  angleDeg?: number;
  radius?: number;
}

/** 视口变换手柄拖动返回的结果。 */
export interface DragTransformResult {
  position?: [number, number, number];
  rotation?: [number, number, number, number];
  scale?: [number, number, number];
  feedback?: DragFeedback;
}

/** 切换视口的当前变换工具模式（"select" | "move" | "rotate" | "scale"）。 */
export function viewportSetTool(id: string, tool: string): Promise<void> {
  return call<void>("editor_viewport_set_tool", { id, tool });
}

/** 切换视口的当前坐标系（"global" | "local"）。 */
export function viewportSetSpace(id: string, space: string): Promise<void> {
  return call<void>("editor_viewport_set_space", { id, space });
}

/** 切换视口地面参考网格的显隐。 */
export function viewportSetGrid(id: string, enabled: boolean): Promise<void> {
  return call<void>("editor_viewport_set_grid", { id, enabled });
}

/** 场景视图点选：`x` / `y` 是 canvas 位图坐标（左上原点），返回命中的实体下标。 */
export function viewportPick(id: string, x: number, y: number): Promise<number | null> {
  return call<number | null>("editor_viewport_pick", { id, x, y });
}

/** 场景视图框选：返回矩形框内所有实体的下标列表。 */
export function viewportBoxSelect(
  id: string,
  minX: number,
  minY: number,
  maxX: number,
  maxY: number,
): Promise<number[]> {
  return call<number[]>("editor_viewport_box_select", {
    id,
    minX,
    minY,
    maxX,
    maxY,
  });
}

/** 实体在视口屏幕上的 3D 投影包围盒。 */
export interface ProjectedBoundBox {
  index: number;
  isPrimary: boolean;
  corners?: [number, number][];
  rect: [number, number, number, number];
}

/** 获取选中实体列表在视口屏幕上的投影包围盒。 */
export function viewportSelectionBounds(
  id: string,
  indices: number[],
): Promise<ProjectedBoundBox[]> {
  return call<ProjectedBoundBox[]>("editor_viewport_selection_bounds", { id, indices });
}

/** 光标下的变换手柄（`"x"` / `"y"` / `"z"` / `"xy"` / `"xz"` / `"yz"` / `"xyz"`，没有则 `null`）。 */
export function viewportGizmoHit(id: string, x: number, y: number): Promise<string | null> {
  return call<string | null>("editor_viewport_gizmo_hit", { id, x, y });
}

/** 鼠标悬停在 Gizmo 手柄上的高亮几何信息。 */
export interface GizmoHoverInfo {
  axis: string;
  tool: string;
  centerScreen: [number, number];
  tipScreen?: [number, number];
  quadScreen?: [number, number][];
  ringPts?: [number, number][];
}

/** 光标悬停在手柄上返回的高亮投影信息（没有悬停手柄则为 null）。 */
export function viewportGizmoHover(
  id: string,
  x: number,
  y: number,
): Promise<GizmoHoverInfo | null> {
  return call<GizmoHoverInfo | null>("editor_viewport_gizmo_hover", { id, x, y });
}

/** 开始拖动选中实体的某根轴。 */
export function viewportDragBegin(
  id: string,
  axis: string,
  x: number,
  y: number,
): Promise<void> {
  return call<void>("editor_viewport_drag_begin", { id, axis, x, y });
}

/** 拖动到新光标位置，返回实体新的局部变换属性（position / rotation / scale）。 */
export function viewportDrag(
  id: string,
  x: number,
  y: number,
  snap = false,
): Promise<DragTransformResult | null> {
  return call<DragTransformResult | null>("editor_viewport_drag", { id, x, y, snap });
}

/** 结束拖动。 */
export function viewportDragEnd(id: string): Promise<void> {
  return call<void>("editor_viewport_drag_end", { id });
}

/** 重新取景（`"all"` / `"selection"`），返回新的轨道参数。 */
export function viewportFrame(
  id: string,
  mode: "all" | "selection",
): Promise<CameraOrbit | null> {
  return call<CameraOrbit | null>("editor_viewport_frame", { id, mode });
}

/** Play 模式暂停 / 继续。 */
export function viewportSetPaused(id: string, paused: boolean): Promise<void> {
  return call<void>("editor_viewport_set_paused", { id, paused });
}

/** 暂停中前进一帧。 */
export function viewportStep(id: string): Promise<void> {
  return call<void>("editor_viewport_step", { id });
}

/** 关闭视口并清理临时快照。 */
export function viewportClose(id: string): Promise<boolean> {
  return call<boolean>("editor_viewport_close", { id });
}
