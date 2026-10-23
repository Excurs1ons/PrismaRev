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

/** 新建场景模板。 */
export interface SceneTemplate {
  id: string;
  label: string;
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
