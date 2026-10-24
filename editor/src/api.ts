import type {
  AssetInfo,
  AssetRef,
  DocKind,
  EntityPreset,
  InstantiateResult,
  JsonValue,
  MoveResult,
  SceneDoc,
  SceneEntry,
  SceneTemplate,
  Schema,
} from "./types";

/**
 * Tauri 命令的类型化包装。
 *
 * 所有场景/预制体/schema 逻辑都在 Rust 侧（`prism-editor-api`）；这里只做
 * `invoke` 的名字与参数映射，前端不重复实现任何 schema 逻辑。
 */

/** 是否运行在 Tauri 宿主里（浏览器直接打开时为 false → 走预览 fixture）。 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri) {
    // 动态 import：Tauri 构建里这一个 chunk 不会被打进主包。
    const mock = await import("./mock");
    return mock.invoke<T>(cmd, args);
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
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

/** 哪些文档字段引用了该资产。 */
export function assetUsage(path: string): Promise<AssetRef[]> {
  return call<AssetRef[]>("editor_asset_usage", { path });
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

/** 把预制体实例化进场景，返回新文档与插入下标。 */
export function instantiatePrefab(
  scene: SceneDoc,
  prefabPath: string,
  parent: number | null,
): Promise<InstantiateResult> {
  return call<InstantiateResult>("editor_instantiate_prefab", { scene, prefabPath, parent });
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
