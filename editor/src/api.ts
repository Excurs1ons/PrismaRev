import type { SceneDoc, SceneEntry, SceneTemplate, Schema } from "./types";

/**
 * Tauri 命令的类型化包装。
 *
 * 所有场景/schema 逻辑都在 Rust 侧（`prism-editor-api`）；这里只做
 * `invoke` 的名字与参数映射，前端不重复实现任何 schema 逻辑。
 */

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

/** 组件属性面板 schema。 */
export function fetchSchema(): Promise<Schema> {
  return call<Schema>("editor_schema");
}

/** 项目根（含 assets/scenes）。 */
export function fetchProjectRoot(): Promise<string> {
  return call<string>("editor_project_root");
}

/** 枚举场景文件。 */
export function listScenes(): Promise<SceneEntry[]> {
  return call<SceneEntry[]>("editor_list_scenes");
}

/** 可用的新建模板。 */
export function fetchTemplates(): Promise<SceneTemplate[]> {
  return call<SceneTemplate[]>("editor_scene_templates");
}

/** 按模板新建场景文档。 */
export function newScene(template: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_new_scene", { template });
}

/** 读取场景文档（已规范化）。 */
export function readScene(path: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_read_scene", { path });
}

/** 校验并写回场景文档。 */
export function writeScene(path: string, doc: SceneDoc): Promise<void> {
  return call<void>("editor_write_scene", { path, doc });
}

/** 另存为新名字（不覆盖），返回新路径。 */
export function writeSceneAs(name: string, doc: SceneDoc): Promise<string> {
  return call<string>("editor_write_scene_as", { name, doc });
}

/** 重命名场景文件，返回新路径。 */
export function renameScene(path: string, name: string): Promise<string> {
  return call<string>("editor_rename_scene", { path, name });
}

/** 删除场景文件。 */
export function deleteScene(path: string): Promise<void> {
  return call<void>("editor_delete_scene", { path });
}

/** 把场景 cook 成 RSCN，返回命令输出。 */
export function cookScene(path: string): Promise<string> {
  return call<string>("editor_cook_scene", { path });
}
