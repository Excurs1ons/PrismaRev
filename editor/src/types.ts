/** 与 `prism-editor-api`（serde camelCase）对齐的前端类型。 */

/** 字段控件类型：`bool` / `i32` / `f32` / `string` / `struct` / `list` / `option` / `array`。 */
export interface FieldSchema {
  name: string;
  kind: string;
  structType: string | null;
  inner: FieldSchema | null;
  arrayLen: number | null;
  readOnly: boolean;
}

export interface TypeSchema {
  name: string;
  typeId: string;
  version: number;
  isComponent: boolean;
  fields: FieldSchema[];
  defaultJson: unknown | null;
}

export interface Schema {
  types: TypeSchema[];
}

/** 可打开的场景文件。 */
export interface SceneEntry {
  name: string;
  fileName: string;
  path: string;
}

/** 文档种类：场景 / 预制体（同一作者 JSON 形状）。 */
export type DocKind = "scene" | "prefab";

/** 新建文档模板。 */
export interface SceneTemplate {
  id: string;
  label: string;
}

/** 预制体实例化结果。 */
export interface InstantiateResult {
  doc: SceneDoc;
  inserted: number[];
}

/** 层级移动结果（新文档 + 实体新下标）。 */
export interface MoveResult {
  doc: SceneDoc;
  index: number;
}

/** 组件剪贴板。 */
export interface ComponentClipboard {
  component: string;
  value: JsonValue;
}

/**
 * 预制体实例链接组件名（编辑器元数据，见 `prism-scene` 的 `PrefabInstance`）。
 * 引擎运行时不查询它。
 */
export const PREFAB_LINK_COMPONENT = "prism_engine::scene::PrefabInstance";

/** 一条预制体实例反向链接。 */
export interface PrefabLink {
  index: number;
  prefab: string;
}

/** 资产元信息（资产管理器用）。 */
export interface AssetInfo {
  path: string;
  sizeBytes: number;
  kind: "model" | "texture" | "env" | "scene" | "package" | "data" | "other";
}

/** 文本资产预览（后端按 UTF-8 读取，可截断）。 */
export interface AssetTextPreview {
  path: string;
  text: string;
  truncated: boolean;
  sizeBytes: number;
}

/** 资产文件操作结果（重命名 / 移动 / 删除）。 */
export interface AssetMutation {
  from: string;
  to: string | null;
  referencesUpdated: number;
  affectedDocuments: string[];
}

/** 「某场景某字段引用了该资产」的记录。 */
export interface AssetRef {
  scene: string;
  scenePath: string;
  entity: string;
  entityIndex: number;
  component: string;
  field: string;
  value: string;
}

/** 快速添加实体的预设。 */
export interface EntityPreset {
  id: string;
  label: string;
  name: string;
  components: string[];
  position: number[];
}

export interface TransformJson {
  position: number[];
  rotation: number[];
  scale: number[];
}

/** `.scene.json` 的一个实体。 */
export interface EntityJson {
  name?: string | null;
  parent?: number | null;
  transform: TransformJson;
  components: Record<string, unknown>;
}

/** `.scene.json` 根文档。 */
export interface SceneDoc {
  version: number;
  entities: EntityJson[];
}

/** 任意 JSON 值。 */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

/** `assets/scenes.toml` 中的一个可运行场景。 */
export interface PlayableScene {
  name: string;
  path: string;
  /** 对应作者文档（存在时）。 */
  docPath: string | null;
}

/** 运行时场景包（`assets/scenes.pak`）状态。 */
export interface ScenePackageStatus {
  exists: boolean;
  sizeBytes: number;
}

/** 游戏进程 stdout/stderr 的一行。 */
export interface PreviewLogLine {
  stream: "stdout" | "stderr" | "system" | string;
  line: string;
}

/** 项目级渲染设置（`assets/settings.toml` 的 `[render]`）。 */
export interface RenderSettingsData {
  renderMode: "raster" | "path_trace";
  debugFlags: number;
  tonemapMode: number;
  debugRt: number;
  ptMaxBounces: number;
  ptRayMaxDistance: number;
  ptMaxIterations: number;
}

/** Build Settings 中的一行场景（顺序即构建/启动顺序）。 */
export interface SceneListEntry {
  name: string;
  path: string;
  enabled: boolean;
}

/** 项目设置（`assets/settings.toml` 的 app/window/build 段）。 */
export interface ProjectSettingsData {
  app: {
    company: string;
    name: string;
    version: string;
  };
  window: {
    title: string;
    width: number;
    height: number;
    resizable: boolean;
    fullscreen: boolean;
    vsync: boolean;
  };
  build: {
    target: string;
    profile: string;
    outputDir: string;
    runAfter: boolean;
  };
}

export interface BuildProjectOptions {
  target: string;
  profile: string;
  outputDir: string;
  runAfter: boolean;
}

export interface BuildProjectResult {
  output: string;
  exe: string;
  outputDir: string;
  durationMs: number;
}

/** 离线 GI 烘焙参数（`prism-build-pipeline bake-gi`）。 */
export interface BakeGiOptions {
  rays: number;
  bounces: number;
  output: string;
  pak: string;
  rscn: string;
}

/** 程序化高度图参数（`prism-build-pipeline heightmap`）。 */
export interface HeightmapOptions {
  output: string;
  width: number;
  height: number;
  seed: number;
  iterations: number;
  particles: number;
  seaLevel: number;
}

// ── Unity 形状的门面（AssetDatabase / PrefabUtility / SceneManagement / Selection / Application） ──

/** 资产分类（同 `AssetInfo["kind"]`）= `AssetDatabase.GetMainAssetTypeAtPath`。 */
export type AssetKind = AssetInfo["kind"];

/** `editor_import_asset` 的结果：规范化路径 + 分类。 */
export interface AssetImportInfo {
  path: string;
  kind: string;
}

/** 预制体资产类型 = `PrefabAssetType`。 */
export type PrefabAssetType = "regular" | "variant" | "notAPrefab";

/** 实例链接状态 = `PrefabInstanceStatus`。 */
export type PrefabInstanceStatus = "notAPrefab" | "connected";

/** 一条预制体实例信息 = `PrefabUtility.GetPrefabInstanceStatus` 的批量形态。 */
export interface PrefabInstanceInfo {
  index: number;
  prefab: string;
  status: PrefabInstanceStatus;
}

/** 解包模式 = `PrefabUnpackMode`。 */
export type PrefabUnpackMode = "outermost_root" | "completely";

/** 编辑器场景元信息 = `UnityEngine.SceneManagement.Scene`。 */
export interface SceneMeta {
  name: string;
  path: string;
  guid: string;
  isDirty: boolean;
  isLoaded: boolean;
  rootCount: number;
  entityCount: number;
  buildIndex: number;
}

/** 选择过滤模式 = `UnityEditor.SelectionMode`。 */
export type SelectionMode = "all" | "root" | "prefabRoot" | "hasComponent";

/** 编辑器关键路径 = `Application.dataPath` / `streamingAssetsPath` 等。 */
export interface EditorPaths {
  project: string;
  data: string;
  streamingAssets: string;
  application: string;
}

/** 渲染图报告中的一个 pass。 */
export interface RenderGraphPass {
  index: number;
  name: string;
  kind: string;
  inputs: number[];
  outputs: number[];
}

/** 渲染图报告中的一个图管理资源。 */
export interface RenderGraphResource {
  handle: number;
  name: string;
  kind: string;
  format: string | null;
  width: number | null;
  height: number | null;
  sampleCount: number | null;
  sizeBytes: number | null;
  allocated: boolean;
}

/** 报告里记录的运行时渲染设置。 */
export interface RenderGraphSettings {
  renderMode: string;
  debugFlags: number;
  tonemapMode: number;
  debugRt: number;
  ptMaxBounces: number;
  ptRayMaxDistance: number;
  ptMaxIterations: number;
}

/** 游戏进程写出的实时渲染图快照。 */
export interface RenderGraphReport {
  generatedAtMs: number;
  frame: number;
  settings: RenderGraphSettings;
  passes: RenderGraphPass[];
  resources: RenderGraphResource[];
}
