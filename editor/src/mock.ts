import type {
  AssetInfo,
  AssetRef,
  EntityPreset,
  JsonValue,
  SceneDoc,
  SceneEntry,
  SceneTemplate,
  Schema,
} from "./types";

/**
 * 浏览器预览用的 **只读 fixture 后端**。
 *
 * 只有在「非 Tauri 环境」（即 `pnpm dev` 直接用浏览器打开）时才会被加载：
 * `api.ts` 检测不到 `__TAURI_INTERNALS__` 时改走这里。数据来自
 * `public/demo.json`（由 `cargo run -p prism-editor-api --example dump_fixture`
 * 用真实注册表生成），改动只存在于内存，刷新即复位。
 */

interface Fixture {
  schema: Schema;
  templates: SceneTemplate[];
  presets: EntityPreset[];
  assetInfos: AssetInfo[];
  scenes: SceneEntry[];
  docs: Record<string, SceneDoc>;
  templateDocs: Record<string, SceneDoc>;
}

let cache: Fixture | null = null;

async function fixture(): Promise<Fixture> {
  if (!cache) {
    const response = await fetch("demo.json");
    if (!response.ok) {
      throw new Error("预览 fixture 缺失：请先运行 `pnpm fixture` 生成 editor/public/demo.json");
    }
    const loaded = (await response.json()) as Fixture;
    // 深拷贝，编辑不应污染 fixture 缓存。
    loaded.docs = JSON.parse(JSON.stringify(loaded.docs)) as Record<string, SceneDoc>;
    loaded.templateDocs = JSON.parse(JSON.stringify(loaded.templateDocs)) as Record<
      string,
      SceneDoc
    >;
    cache = loaded;
  }
  return cache;
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function fileNameOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function stemOf(path: string): string {
  return fileNameOf(path).replace(/\.scene\.json$/, "");
}

function dirOf(path: string): string {
  return path.slice(0, path.lastIndexOf("/"));
}

function sortScenes(scenes: SceneEntry[]) {
  scenes.sort((a, b) => a.name.localeCompare(b.name));
}

/** 与 Rust 侧一致的路径归一化（折叠 `.` / `..`）。 */
function resolvePath(base: string, value: string): string {
  const joined = value.startsWith("/") ? value : `${base}/${value}`;
  const parts: string[] = [];
  for (const part of joined.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      parts.pop();
      continue;
    }
    parts.push(part);
  }
  return parts.join("/");
}

function collectStrings(
  value: JsonValue,
  prefix: string,
  visit: (field: string, text: string) => void,
) {
  if (typeof value === "string") {
    visit(prefix, value);
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => collectStrings(item, `${prefix}[${index}]`, visit));
    return;
  }
  if (value && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      collectStrings(item, prefix ? `${prefix}.${key}` : key, visit);
    }
  }
}

function validate(doc: SceneDoc | undefined) {
  if (!doc) throw new Error("空文档");
  if (!Array.isArray(doc.entities) || doc.entities.length === 0) {
    throw new Error("scene must contain at least one entity");
  }
  const count = doc.entities.length;
  doc.entities.forEach((entity, index) => {
    const parent = entity.parent;
    if (parent === null || parent === undefined) return;
    if (parent < 0 || parent >= count) {
      throw new Error(`entity ${index}: parent index ${parent} out of bounds`);
    }
    if (parent === index) {
      throw new Error(`entity ${index}: self-parent not allowed`);
    }
  });
}

async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  const data = await fixture();

  switch (cmd) {
    case "editor_schema":
      return data.schema;
    case "editor_project_root":
      return "/preview（浏览器预览）";
    case "editor_list_scenes":
      sortScenes(data.scenes);
      return data.scenes;
    case "editor_scene_templates":
      return data.templates;
    case "editor_entity_presets":
      return data.presets;
    case "editor_list_assets":
      return data.assetInfos.map((asset) => asset.path);
    case "editor_list_assets_info":
      return data.assetInfos;
    case "editor_validate_scene":
      validate(args.doc as SceneDoc);
      return null;
    case "editor_new_scene": {
      const id = String(args.template ?? "camera");
      return clone(data.templateDocs[id] ?? data.templateDocs.camera);
    }
    case "editor_read_scene": {
      const path = String(args.path);
      const doc = data.docs[path];
      if (!doc) throw new Error(`预览中没有该场景：${path}`);
      return clone(doc);
    }
    case "editor_write_scene": {
      const path = String(args.path);
      data.docs[path] = clone(args.doc as SceneDoc);
      if (!data.scenes.some((s) => s.path === path)) {
        data.scenes.push({ name: stemOf(path), fileName: fileNameOf(path), path });
      }
      return null;
    }
    case "editor_write_scene_as": {
      const name = String(args.name).trim();
      const path = `/preview/assets/scenes/${name.replace(/\.scene\.json$/, "")}.scene.json`;
      if (data.docs[path]) throw new Error("已存在同名场景");
      data.docs[path] = clone(args.doc as SceneDoc);
      data.scenes.push({ name: stemOf(path), fileName: fileNameOf(path), path });
      return path;
    }
    case "editor_rename_scene": {
      const path = String(args.path);
      const name = String(args.name).trim();
      const target = `${dirOf(path)}/${name.replace(/\.scene\.json$/, "")}.scene.json`;
      if (target !== path && data.docs[target]) throw new Error("已存在同名场景");
      const doc = data.docs[path];
      if (!doc) throw new Error("场景不存在");
      delete data.docs[path];
      data.docs[target] = doc;
      const entry = data.scenes.find((s) => s.path === path);
      if (entry) {
        entry.name = stemOf(target);
        entry.fileName = fileNameOf(target);
        entry.path = target;
      }
      return target;
    }
    case "editor_delete_scene": {
      const path = String(args.path);
      delete data.docs[path];
      data.scenes = data.scenes.filter((s) => s.path !== path);
      return null;
    }
    case "editor_asset_usage": {
      const target = resolvePath("/preview", String(args.path));
      const refs: AssetRef[] = [];
      for (const entry of data.scenes) {
        const doc = data.docs[entry.path];
        if (!doc) continue;
        const sceneDir = dirOf(entry.path);
        doc.entities.forEach((entity, index) => {
          for (const [component, payload] of Object.entries(entity.components ?? {})) {
            collectStrings(payload as JsonValue, "", (field, value) => {
              if (!value.trim()) return;
              if (
                resolvePath(sceneDir, value) === target ||
                resolvePath("/preview", value) === target
              ) {
                refs.push({
                  scene: entry.name,
                  scenePath: entry.path,
                  entity: entity.name ?? `#${index}`,
                  entityIndex: index,
                  component,
                  field,
                  value,
                });
              }
            });
          }
        });
      }
      return refs;
    }
    case "editor_set_component_field": {
      const component = String(args.component);
      const path = String(args.path);
      const type = data.schema.types.find((t) => t.name === component);
      if (!type) throw new Error(`'${component}' 不是已注册的可反射组件`);
      if (path) {
        const root = path.split(/[.[]/)[0];
        const field = type.fields.find((f) => f.name === root);
        if (field?.readOnly) {
          throw new Error(`字段 '${root}' 写入被拒绝: field '${root}' is read-only`);
        }
      }
      // 预览模式不做真正的反射往返，仅回显（只读检查已尽量对齐 Rust 行为）。
      return clone(args.after as JsonValue);
    }
    case "editor_cook_scene":
      return "（浏览器预览模式：未真正调用 prism-build-pipeline）\n请用 `cd editor && pnpm tauri dev` 或 `prism-build-pipeline` 执行。";
    default:
      throw new Error(`浏览器预览模式不支持命令 ${cmd}`);
  }
}

export function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return handle(cmd, args) as Promise<T>;
}
