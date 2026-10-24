import type {
  AssetInfo,
  AssetMutation,
  AssetRef,
  AssetTextPreview,
  BakeGiOptions,
  BuildProjectOptions,
  BuildProjectResult,
  DocKind,
  EntityPreset,
  HeightmapOptions,
  InstantiateResult,
  JsonValue,
  MoveResult,
  PlayableScene,
  PrefabLink,
  ProjectSettingsData,
  PreviewLogLine,
  RenderGraphReport,
  RenderSettingsData,
  SceneDoc,
  SceneEntry,
  ScenePackageStatus,
  SceneTemplate,
  Schema,
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

/** 运行时场景包状态。 */
export function scenePackageStatus(): Promise<ScenePackageStatus> {
  return call<ScenePackageStatus>("editor_scene_package_status");
}

/** 重新 cook + 打包所有场景（`build-scenes`），返回命令输出。 */
export function buildScenes(): Promise<string> {
  return call<string>("editor_build_scenes");
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

/** 写入项目级渲染设置。 */
export function writeRenderSettings(settings: RenderSettingsData): Promise<void> {
  return call<void>("editor_write_render_settings", { settings });
}

/** 读取项目设置（app/window/build）。 */
export function projectSettings(): Promise<ProjectSettingsData> {
  return call<ProjectSettingsData>("editor_project_settings");
}

/** 写入项目设置（保留 [render]）。 */
export function writeProjectSettings(settings: ProjectSettingsData): Promise<void> {
  return call<void>("editor_write_project_settings", { settings });
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
