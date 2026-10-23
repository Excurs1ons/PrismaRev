import type { SceneEntry, SceneDoc, Schema } from "./types";

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

/** 读取场景文档。 */
export function readScene(path: string): Promise<SceneDoc> {
  return call<SceneDoc>("editor_read_scene", { path });
}

/** 校验并写回场景文档。 */
export function writeScene(path: string, doc: SceneDoc): Promise<void> {
  return call<void>("editor_write_scene", { path, doc });
}

/** 新建场景文档。 */
export function newScene(): Promise<SceneDoc> {
  return call<SceneDoc>("editor_new_scene");
}
