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

/** 资产元信息（资产管理器用）。 */
export interface AssetInfo {
  path: string;
  sizeBytes: number;
  kind: "model" | "texture" | "env" | "scene" | "package" | "data" | "other";
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
  translation: number[];
}

export interface TransformJson {
  translation: number[];
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
