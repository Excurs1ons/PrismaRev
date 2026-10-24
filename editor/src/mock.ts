import { PREFAB_LINK_COMPONENT } from "./types";
import type {
  AssetInfo,
  AssetMutation,
  AssetRef,
  DocKind,
  EntityJson,
  EntityPreset,
  InstantiateResult,
  JsonValue,
  MoveResult,
  PlayableScene,
  PrefabLink,
  ProjectSettingsData,
  RenderGraphReport,
  RenderSettingsData,
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
  presets: EntityPreset[];
  assetInfos: AssetInfo[];
  documents: Record<DocKind, SceneEntry[]>;
  docs: Record<string, SceneDoc>;
  templatesByKind: Record<DocKind, SceneTemplate[]>;
  templateDocs: Record<DocKind, Record<string, SceneDoc>>;
}

let cache: Fixture | null = null;

/** 浏览器预览里新建的空目录（真实后端会从磁盘枚举）。 */
const extraDirs: string[] = [];

function derivedDirs(infos: AssetInfo[]): string[] {
  const dirs = new Set<string>();
  for (const asset of infos) {
    const parts = asset.path.split("/");
    parts.pop();
    let current = "";
    for (const part of parts) {
      current = current ? `${current}/${part}` : part;
      dirs.add(current);
    }
  }
  for (const dir of extraDirs) dirs.add(dir);
  return [...dirs].sort();
}

/** 浏览器预览用的渲染设置（仅内存）。 */
const renderSettingsFixture: RenderSettingsData = {
  renderMode: "raster",
  debugFlags: 0,
  tonemapMode: 0,
  debugRt: 0,
  ptMaxBounces: 3,
  ptRayMaxDistance: 1000,
  ptMaxIterations: 0,
};

const projectSettingsFixture: ProjectSettingsData = {
  app: { company: "Excurs1ons", name: "PrismaRev", version: "0.1.0" },
  window: {
    title: "PrismaRev",
    width: 1600,
    height: 900,
    resizable: true,
    fullscreen: false,
    vsync: true,
  },
  build: {
    target: "windows",
    profile: "release",
    outputDir: "build/windows",
    runAfter: false,
  },
};

async function fixture(): Promise<Fixture> {
  if (!cache) {
    const response = await fetch("demo.json");
    if (!response.ok) {
      throw new Error("预览 fixture 缺失：请先运行 `pnpm fixture` 生成 editor/public/demo.json");
    }
    const loaded = (await response.json()) as Fixture;
    // 深拷贝，编辑不应污染 fixture 缓存。
    loaded.docs = JSON.parse(JSON.stringify(loaded.docs)) as Record<string, SceneDoc>;
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

function stemOf(path: string, kind: DocKind): string {
  return fileNameOf(path).replace(new RegExp(`\\.${kind === "scene" ? "scene" : "prefab"}\\.json$`), "");
}

function dirOf(path: string): string {
  return path.slice(0, path.lastIndexOf("/"));
}

function sortEntries(entries: SceneEntry[]) {
  entries.sort((a, b) => a.name.localeCompare(b.name));
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
    throw new Error("文档至少要有一个实体");
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

/** 提取「根实体 + 后代」为预制体（与 Rust 的 extract_subtree 同语义）。 */
function extractSubtree(doc: SceneDoc, root: number): SceneDoc {
  validate(doc);
  if (root < 0 || root >= doc.entities.length) throw new Error(`实体下标 ${root} 越界`);

  const order: number[] = [];
  const stack = [root];
  while (stack.length) {
    const index = stack.pop() as number;
    if (order.includes(index)) continue;
    order.push(index);
    for (let child = doc.entities.length - 1; child >= 0; child--) {
      if (doc.entities[child].parent === index && !order.includes(child)) stack.push(child);
    }
  }

  const mapping = new Map<number, number>();
  order.forEach((oldIndex, newIndex) => mapping.set(oldIndex, newIndex));

  const entities = order.map((oldIndex) => {
    const entity = clone(doc.entities[oldIndex]);
    const parent = entity.parent;
    entity.parent =
      parent === null || parent === undefined ? null : (mapping.get(parent) ?? null);
    return entity;
  });

  // 预制体不该记录「自己是哪个预制体的实例」。
  if (entities[0]?.components) {
    delete entities[0].components[PREFAB_LINK_COMPONENT];
  }

  return { version: 1, entities };
}

/** 实例化预制体（与 Rust 的 instantiate_prefab 同语义，含反向链接）。 */
function instantiatePrefab(
  scene: SceneDoc,
  prefab: SceneDoc,
  parent: number | null,
  link: string | null,
): InstantiateResult {
  validate(scene);
  validate(prefab);
  if (parent !== null && (parent < 0 || parent >= scene.entities.length)) {
    throw new Error(`父实体下标 ${parent} 越界`);
  }

  const base = scene.entities.length;
  const names = scene.entities.map((entity) => entity.name ?? "");
  const entities = scene.entities.map((entity) => clone(entity));
  const inserted: number[] = [];

  for (const prefabEntity of prefab.entities) {
    const entity = clone(prefabEntity);
    const baseName = entity.name ?? "entity";
    let name = baseName;
    let suffix = 2;
    while (names.includes(name)) {
      name = `${baseName}_${suffix}`;
      suffix += 1;
    }
    names.push(name);
    entity.name = name;

    const prefabParent = prefabEntity.parent;
    const isRoot = prefabParent === null || prefabParent === undefined;
    entity.parent = isRoot ? parent : base + prefabParent;

    if (isRoot && link) {
      entity.components = {
        ...(entity.components ?? {}),
        [PREFAB_LINK_COMPONENT]: { prefab: link },
      };
    }

    inserted.push(entities.length);
    entities.push(entity);
  }

  return { doc: { version: scene.version ?? 1, entities }, inserted };
}

/** 子树下标（DFS，根在首位）。 */
function subtreeIndices(doc: SceneDoc, root: number): number[] {
  const order: number[] = [];
  const stack = [root];
  while (stack.length) {
    const index = stack.pop() as number;
    if (order.includes(index)) continue;
    order.push(index);
    for (let child = doc.entities.length - 1; child >= 0; child--) {
      if (doc.entities[child].parent === index && !order.includes(child)) stack.push(child);
    }
  }
  return order;
}

function prefabLinks(doc: SceneDoc): PrefabLink[] {
  const links: PrefabLink[] = [];
  doc.entities.forEach((entity, index) => {
    const link = entity.components?.[PREFAB_LINK_COMPONENT] as { prefab?: string } | undefined;
    if (typeof link?.prefab === "string") links.push({ index, prefab: link.prefab });
  });
  return links;
}

/** 把某个实例根还原成预制体当前内容（与 Rust 的 revert_prefab_instance 同语义）。 */
function revertPrefabInstance(
  doc: SceneDoc,
  root: number,
  prefab: SceneDoc,
  link: string | null,
): MoveResult {
  validate(doc);
  if (root < 0 || root >= doc.entities.length) throw new Error(`实体下标 ${root} 越界`);

  const keepName = doc.entities[root].name ?? null;
  const parent = doc.entities[root].parent;
  const subtree = subtreeIndices(doc, root);
  const newParent =
    parent === null || parent === undefined
      ? null
      : parent - subtree.filter((index) => index < parent).length;

  const trimmed = deleteEntitiesInDoc(doc, subtree);
  const result = instantiatePrefab(trimmed, prefab, newParent, link);
  const newRoot = result.inserted[0];
  if (keepName !== null && newRoot !== undefined) {
    result.doc.entities[newRoot].name = keepName;
  }
  return { doc: result.doc, index: newRoot ?? root };
}

/** `/preview/assets/prefabs/x.prefab.json` → `assets/prefabs/x.prefab.json` */
function rootRelative(path: string): string {
  return path.replace(/^\/preview\//, "");
}

function isDescendant(entities: EntityJson[], node: number, ancestor: number): boolean {
  let current: number | null = node;
  let guard = 0;
  while (current !== null) {
    if (current === ancestor) return true;
    guard += 1;
    if (guard > entities.length) return false;
    const parent: number | null | undefined = entities[current]?.parent;
    current = parent === null || parent === undefined ? null : parent;
  }
  return false;
}

/** 与 Rust 的 move_entity 同语义：移动数组位置 + 设置父级 + 全量重映射。 */
function moveEntityInDoc(
  doc: SceneDoc,
  child: number,
  at: number,
  parent: number | null,
): MoveResult {
  validate(doc);
  const count = doc.entities.length;
  if (child < 0 || child >= count) throw new Error(`实体下标 ${child} 越界`);
  if (at < 0 || at > count) throw new Error(`插入位置 ${at} 越界`);
  if (parent !== null) {
    if (parent < 0 || parent >= count) throw new Error(`父实体下标 ${parent} 越界`);
    if (parent === child) throw new Error("不能以自己为父");
    if (isDescendant(doc.entities, parent, child)) {
      throw new Error("不能挂到自己的后代下（会成环）");
    }
  }

  const order = doc.entities.map((_, index) => index);
  order.splice(child, 1);
  order.splice(Math.min(at > child ? at - 1 : at, order.length), 0, child);

  const remap = new Array<number>(count).fill(-1);
  order.forEach((oldIndex, newIndex) => {
    remap[oldIndex] = newIndex;
  });

  const entities = order.map((oldIndex) => {
    const entity = clone(doc.entities[oldIndex]);
    const oldParent = entity.parent;
    entity.parent =
      oldIndex === child
        ? parent === null
          ? null
          : remap[parent]
        : oldParent === null || oldParent === undefined
          ? null
          : remap[oldParent];
    return entity;
  });

  return { doc: { version: doc.version ?? 1, entities }, index: remap[child] };
}

/** 与 Rust 的 delete_entities 同语义。 */
function deleteEntitiesInDoc(doc: SceneDoc, indices: number[]): SceneDoc {
  validate(doc);
  const count = doc.entities.length;
  const removed = new Array<boolean>(count).fill(false);
  for (const index of indices) {
    if (index < 0 || index >= count) throw new Error(`实体下标 ${index} 越界`);
    removed[index] = true;
  }
  if (removed.every(Boolean)) throw new Error("至少要保留一个实体");

  const remap = new Array<number>(count).fill(-1);
  let next = 0;
  removed.forEach((isRemoved, oldIndex) => {
    if (!isRemoved) remap[oldIndex] = next++;
  });

  const entities: EntityJson[] = [];
  doc.entities.forEach((entity, oldIndex) => {
    if (removed[oldIndex]) return;
    const copy = clone(entity);
    const oldParent = copy.parent;
    copy.parent =
      oldParent === null || oldParent === undefined || removed[oldParent]
        ? null
        : remap[oldParent];
    entities.push(copy);
  });

  return { version: doc.version ?? 1, entities };
}

function documentPath(kind: DocKind, name: string): string {
  const dir = kind === "scene" ? "scenes" : "prefabs";
  const suffix = kind === "scene" ? "scene.json" : "prefab.json";
  return `/preview/assets/${dir}/${name.replace(/\.(scene|prefab)?\.?json$/, "")}.${suffix}`;
}

async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  const data = await fixture();

  switch (cmd) {
    case "editor_schema":
      return data.schema;
    case "editor_project_root":
      return "/preview（浏览器预览）";
    case "editor_entity_presets":
      return data.presets;
    case "editor_list_assets":
      return data.assetInfos.map((asset) => asset.path);
    case "editor_list_assets_info":
      return data.assetInfos;
    case "editor_validate_document":
      validate(args.doc as SceneDoc);
      return null;

    // ── 文档 ────────────────────────────────────────────────────────────
    case "editor_list_documents": {
      const kind = String(args.kind) as DocKind;
      sortEntries(data.documents[kind]);
      return data.documents[kind];
    }
    case "editor_document_templates":
      return data.templatesByKind[String(args.kind) as DocKind] ?? [];
    case "editor_new_document": {
      const kind = String(args.kind) as DocKind;
      const template = String(args.template ?? "");
      const table = data.templateDocs[kind] ?? {};
      const doc = table[template] ?? Object.values(table)[0];
      return clone(doc);
    }
    case "editor_read_document": {
      const path = String(args.path);
      const doc = data.docs[path];
      if (!doc) throw new Error(`预览中没有该文档：${path}`);
      return clone(doc);
    }
    case "editor_write_document": {
      data.docs[String(args.path)] = clone(args.doc as SceneDoc);
      return null;
    }
    case "editor_write_document_as": {
      const kind = String(args.kind) as DocKind;
      const path = documentPath(kind, String(args.name));
      if (data.docs[path]) throw new Error("已存在同名文档");
      data.docs[path] = clone(args.doc as SceneDoc);
      data.documents[kind].push({ name: stemOf(path, kind), fileName: fileNameOf(path), path });
      return path;
    }
    case "editor_rename_document": {
      const kind = String(args.kind) as DocKind;
      const path = String(args.path);
      const target = documentPath(kind, String(args.name));
      if (target !== path && data.docs[target]) throw new Error("已存在同名文档");
      const doc = data.docs[path];
      if (!doc) throw new Error("文档不存在");
      delete data.docs[path];
      data.docs[target] = doc;
      const entry = data.documents[kind].find((item) => item.path === path);
      if (entry) {
        entry.name = stemOf(target, kind);
        entry.fileName = fileNameOf(target);
        entry.path = target;
      }
      return target;
    }
    case "editor_delete_document": {
      const path = String(args.path);
      delete data.docs[path];
      (["scene", "prefab"] as DocKind[]).forEach((kind) => {
        data.documents[kind] = data.documents[kind].filter((item) => item.path !== path);
      });
      return null;
    }

    // ── 层级操作 ────────────────────────────────────────────────────────
    case "editor_reparent_entity": {
      const doc = args.doc as SceneDoc;
      const child = Number(args.child);
      const parent = args.parent === null || args.parent === undefined ? null : Number(args.parent);
      return moveEntityInDoc(doc, child, child, parent).doc;
    }
    case "editor_move_entity": {
      const doc = args.doc as SceneDoc;
      const child = Number(args.child);
      const at = Number(args.at);
      const parent = args.parent === null || args.parent === undefined ? null : Number(args.parent);
      return moveEntityInDoc(doc, child, at, parent);
    }
    case "editor_delete_entities":
      return deleteEntitiesInDoc(args.doc as SceneDoc, (args.indices as number[]) ?? []);

    // ── 预制体 ──────────────────────────────────────────────────────────
    case "editor_create_prefab_from_entity": {
      const doc = args.doc as SceneDoc;
      const prefab = extractSubtree(doc, Number(args.entityIndex));
      const path = documentPath("prefab", String(args.name));
      if (data.docs[path]) throw new Error("已存在同名预制体");
      data.docs[path] = prefab;
      data.documents.prefab.push({
        name: stemOf(path, "prefab"),
        fileName: fileNameOf(path),
        path,
      });
      return path;
    }
    case "editor_create_prefab_at": {
      const doc = args.doc as SceneDoc;
      const prefab = extractSubtree(doc, Number(args.entityIndex));
      const folder = String(args.folder ?? "assets/prefabs").replace(/\/$/, "");
      const name = String(args.name || "prefab");
      const path = `${folder}/${name}.prefab.json`;
      if (data.docs[path]) throw new Error("已存在同名预制体");
      data.docs[path] = prefab;
      data.documents.prefab.push({
        name: stemOf(path, "prefab"),
        fileName: fileNameOf(path),
        path,
      });
      return path;
    }
    case "editor_instantiate_prefab": {
      const prefabPath = String(args.prefabPath);
      const prefab = data.docs[prefabPath];
      if (!prefab) throw new Error(`预览中没有该预制体：${prefabPath}`);
      const parent = args.parent === null || args.parent === undefined ? null : Number(args.parent);
      return instantiatePrefab(args.scene as SceneDoc, prefab, parent, rootRelative(prefabPath));
    }
    case "editor_prefab_instances":
      return prefabLinks(args.doc as SceneDoc);
    case "editor_revert_prefab_instance": {
      const link = String(args.prefab);
      const prefab = data.docs[`/preview/${link}`];
      if (!prefab) throw new Error(`预览中没有该预制体：${link}`);
      return revertPrefabInstance(args.doc as SceneDoc, Number(args.rootIndex), prefab, link);
    }
    case "editor_sync_prefab_instances": {
      const link = String(args.prefab);
      const prefab = data.docs[`/preview/${link}`];
      if (!prefab) throw new Error(`预览中没有该预制体：${link}`);
      let doc = args.doc as SceneDoc;
      const roots = prefabLinks(doc)
        .filter((item) => item.prefab === link)
        .map((item) => item.index)
        .sort((a, b) => b - a);
      for (const root of roots) {
        doc = revertPrefabInstance(doc, root, prefab, link).doc;
      }
      return doc;
    }
    case "editor_unlink_prefab_instance": {
      const doc = clone(args.doc as SceneDoc);
      const root = Number(args.rootIndex);
      if (doc.entities[root]?.components) {
        delete doc.entities[root].components[PREFAB_LINK_COMPONENT];
      }
      return doc;
    }
    case "editor_extract_subtree":
      return extractSubtree(args.doc as SceneDoc, Number(args.rootIndex));

    // ── 反射字段写入 ─────────────────────────────────────────────────────
    case "editor_set_component_field": {
      const component = String(args.component);
      const path = String(args.path);
      const type = data.schema.types.find((item) => item.name === component);
      if (!type) throw new Error(`'${component}' 不是已注册的可反射组件`);
      if (path) {
        const root = path.split(/[.[]/)[0];
        const field = type.fields.find((item) => item.name === root);
        if (field?.readOnly) {
          throw new Error(`字段 '${root}' 写入被拒绝: field '${root}' is read-only`);
        }
      }
      // 预览模式不做真正的反射往返，仅回显（只读检查已尽量对齐 Rust 行为）。
      return clone(args.after as JsonValue);
    }

    // ── 资产文件操作（浏览器预览只改内存） ───────────────────────────────
    case "editor_create_asset_dir": {
      const path = String(args.path);
      extraDirs.push(path);
      return path;
    }

    case "editor_list_asset_dirs":
      return derivedDirs(data.assetInfos);

    case "editor_rename_asset":
    case "editor_move_asset": {
      const from = String(args.from);
      const to = String(args.to);
      for (const asset of data.assetInfos) {
        if (asset.path === from || asset.path.startsWith(`${from}/`)) {
          asset.path = to + asset.path.slice(from.length);
        }
      }
      for (let i = 0; i < extraDirs.length; i += 1) {
        if (extraDirs[i] === from || extraDirs[i].startsWith(`${from}/`)) {
          extraDirs[i] = to + extraDirs[i].slice(from.length);
        }
      }
      return {
        from,
        to,
        referencesUpdated: 0,
        affectedDocuments: [],
      } satisfies AssetMutation;
    }

    case "editor_asset_references":
      return [];

    case "editor_delete_asset": {
      const path = String(args.path);
      data.assetInfos = data.assetInfos.filter(
        (asset) => asset.path !== path && !asset.path.startsWith(`${path}/`),
      );
      for (let i = extraDirs.length - 1; i >= 0; i -= 1) {
        if (extraDirs[i] === path || extraDirs[i].startsWith(`${path}/`)) {
          extraDirs.splice(i, 1);
        }
      }
      return {
        from: path,
        to: null,
        referencesUpdated: 0,
        affectedDocuments: [],
      } satisfies AssetMutation;
    }

    case "editor_read_asset_text":
      return {
        path: String(args.path ?? ""),
        text: "（浏览器预览模式不读取真实文件；请用 `pnpm tauri dev` 查看文本/脚本内容）",
        truncated: false,
        sizeBytes: 0,
      };

    case "editor_asset_usage": {
      const target = resolvePath("/preview", String(args.path));
      const refs: AssetRef[] = [];
      for (const kind of ["scene", "prefab"] as DocKind[]) {
        for (const entry of data.documents[kind]) {
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
      }
      return refs;
    }

    case "editor_cook_scene":
      return "（浏览器预览模式：未真正调用 prism-build-pipeline）\n请用 `cd editor && pnpm tauri dev` 或 `prism-build-pipeline` 执行。";
    case "editor_render_graph_report":
      return renderGraphFixture();

    case "editor_list_playable_scenes": {
      const data = await fixture();
      return data.documents.scene.map<PlayableScene>((entry) => ({
        name: entry.name,
        path: `scenes/${entry.name}.rscn`,
        docPath: entry.path,
      }));
    }

    case "editor_scene_package_status":
      return { exists: false, sizeBytes: 0 };

    case "editor_render_settings":
      return { ...renderSettingsFixture };

    case "editor_write_render_settings": {
      Object.assign(renderSettingsFixture, args.settings as RenderSettingsData);
      return null;
    }

    case "editor_project_settings":
      return JSON.parse(JSON.stringify(projectSettingsFixture));

    case "editor_write_project_settings": {
      Object.assign(projectSettingsFixture, args.settings as ProjectSettingsData);
      return null;
    }

    case "editor_build_scenes":
      return "（浏览器预览模式：未真正调用 build-scenes）请用 `cd editor && pnpm tauri dev`。";

    case "editor_build_project":
      return {
        output: "（浏览器预览模式：未真正构建游戏）请用 `pnpm tauri dev`。",
        exe: "build/windows/prisma.exe",
        outputDir: "build/windows",
        durationMs: 0,
      };

    case "editor_reveal_path":
      return null;

    case "editor_bake_gi":
      return "（浏览器预览模式：未真正调用 bake-gi）请用 `pnpm tauri dev` 或手动执行 prism-build-pipeline bake-gi。";
    case "editor_generate_heightmap":
      return "（浏览器预览模式：未真正调用 heightmap）请用 `pnpm tauri dev` 或手动执行 prism-build-pipeline heightmap。";

    case "editor_preview_logs":
      return [];
    case "editor_play_scene":
    case "editor_stop_preview":
    case "editor_preview_running":
      throw new Error("浏览器预览模式无法启动游戏；请用 `pnpm tauri dev`");

    default:
      throw new Error(`浏览器预览模式不支持命令 ${cmd}`);
  }
}

export function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return handle(cmd, args) as Promise<T>;
}


/** 浏览器预览用的静态渲染图样例（与真实 Raster 管线同构）。 */
function renderGraphFixture(): RenderGraphReport {
  const color = 1002;
  const depth = 1000;
  const normal = 1001;
  const ptColor = 1003;
  return {
    generatedAtMs: Date.now(),
    frame: 1234,
    settings: {
      renderMode: "raster",
      debugFlags: 0,
      tonemapMode: 0,
      debugRt: 0,
      ptMaxBounces: 3,
      ptRayMaxDistance: 1000,
      ptMaxIterations: 0,
    },
    passes: [
      { index: 0, name: "ShadowMapPass", kind: "shadow", inputs: [], outputs: [2000] },
      { index: 1, name: "ForwardPass", kind: "forward", inputs: [], outputs: [depth, normal, color] },
      { index: 2, name: "GtaoPass", kind: "gtao", inputs: [depth, normal], outputs: [] },
      { index: 3, name: "PostPass", kind: "post", inputs: [color], outputs: [] },
      { index: 4, name: "PathTracePass", kind: "pt", inputs: [], outputs: [ptColor] },
    ],
    resources: [
      { handle: 1000, name: "forward.depth", kind: "depth", format: null, width: 1600, height: 900, sampleCount: 1, sizeBytes: null, allocated: true },
      { handle: 1001, name: "forward.normal", kind: "color", format: "R16G16B16A16_SFLOAT", width: 1600, height: 900, sampleCount: 1, sizeBytes: null, allocated: true },
      { handle: 1002, name: "forward.color", kind: "color", format: "R16G16B16A16_SFLOAT", width: 1600, height: 900, sampleCount: 1, sizeBytes: null, allocated: true },
      { handle: 1003, name: "pt.color", kind: "storage_image", format: "R32G32B32A32_SFLOAT", width: 1600, height: 900, sampleCount: null, sizeBytes: null, allocated: false },
      { handle: 2000, name: "resource#2000", kind: "depth", format: null, width: 2048, height: 2048, sampleCount: 1, sizeBytes: null, allocated: true },
    ],
  };
}
