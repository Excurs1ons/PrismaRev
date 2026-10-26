<script setup lang="ts">
import { computed, inject, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { ChevronDown, ChevronRight, Plus } from "lucide-vue-next";
import * as api from "../api";
import { ASSETS_KEY } from "../keys";
import { formatBytes } from "../format";
import { clampMenuPosition } from "../contextMenu";
import type { AssetInfo, AssetMutation, AssetRef } from "../types";

/**
 * 侧栏「资产」面板：目录树 / 平铺列表 + 检索 + 引用位置 + 拖拽到字段，
 * 以及 `assets/` 内的文件管理（新建目录 / 重命名 / 移动 / 删除）。
 *
 * 文件操作全部走 `prism-editor-api`：重命名/移动会自动改写场景与预制体里的
 * 路径引用；删除前会先列出受影响文档。
 */
const props = defineProps<{
  /** 写操作前确保当前文档已保存（由 App 注入）。 */
  beforeChange?: () => Promise<boolean>;
}>();

const emit = defineEmits<{
  (e: "jump", ref: AssetRef): void;
  (e: "changed", affected: string[]): void;
  /** 双击场景/预制体：请求宿主打开对应文档。 */
  (e: "open", path: string): void;
  /** 从层级树拖入资产目录：把实体子树保存为预制体。 */
  (e: "prefab-drop", folder: string, entityIndex: number, name: string): void;
}>();

const assets = inject(ASSETS_KEY, null);

type ViewMode = "tree" | "list";
const search = ref("");
const kind = ref("all");
/** 检索或类型过滤生效时自动用平铺列表（结果跨目录）。 */
const isFiltering = computed(
  () => search.value.trim().length > 0 || kind.value !== "all",
);
const currentView = computed<ViewMode>(() =>
  isFiltering.value ? "list" : "tree",
);
const selected = ref<AssetInfo | null>(null);
const selectedNode = ref<{ path: string; name: string; dir: boolean } | null>(null);
const assetMenu = ref<{ path: string; name: string; dir: boolean; x: number; y: number } | null>(null);
const assetDirs = ref<string[]>([]);
const usage = ref<AssetRef[]>([]);
const usageError = ref("");
const busy = ref(false);
const opBusy = ref(false);
const opError = ref("");

// ---------------------------------------------------------------------------
// 预览：图片走原始字节 + Blob URL；文本走 UTF-8 字符串；场景/预制体可打开
// ---------------------------------------------------------------------------

type PreviewKind = "image" | "text" | "document" | "none";

const IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
  avif: "image/avif",
};

const TEXT_EXT = new Set([
  "txt", "md", "json", "toml", "ron", "yaml", "yml", "xml", "csv", "ini", "cfg", "log",
  "slang", "wgsl", "glsl", "vert", "frag", "comp", "hlsl", "shader",
  "gltf", "obj", "mtl",
]);

const previewKind = computed<PreviewKind>(() =>
  selected.value ? previewKindOf(selected.value.path) : "none",
);
const previewUrl = ref("");
const previewText = ref("");
const previewTruncated = ref(false);
const previewError = ref("");
const previewBusy = ref(false);
let previewObjectUrl: string | null = null;
let previewToken = 0;

function extensionOf(path: string): string {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

function isDocumentPath(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.endsWith(".scene.json") || lower.endsWith(".prefab.json");
}

function previewKindOf(path: string): PreviewKind {
  if (isDocumentPath(path)) return "document";
  const ext = extensionOf(path);
  if (ext in IMAGE_MIME) return "image";
  if (TEXT_EXT.has(ext)) return "text";
  return "none";
}

function clearPreview() {
  previewToken += 1;
  if (previewObjectUrl) {
    URL.revokeObjectURL(previewObjectUrl);
    previewObjectUrl = null;
  }
  previewUrl.value = "";
  previewText.value = "";
  previewTruncated.value = false;
  previewError.value = "";
  previewBusy.value = false;
}

async function loadPreview(asset: AssetInfo) {
  const token = ++previewToken;
  if (previewObjectUrl) {
    URL.revokeObjectURL(previewObjectUrl);
    previewObjectUrl = null;
  }
  previewUrl.value = "";
  previewText.value = "";
  previewTruncated.value = false;
  previewError.value = "";
  previewBusy.value = true;

  try {
    if (previewKindOf(asset.path) === "image") {
      const bytes = await api.readAssetBytes(asset.path);
      if (token !== previewToken) return;
      const blob = new Blob([bytes], { type: IMAGE_MIME[extensionOf(asset.path)] });
      previewObjectUrl = URL.createObjectURL(blob);
      previewUrl.value = previewObjectUrl;
    } else if (previewKindOf(asset.path) === "text") {
      const result = await api.readAssetText(asset.path);
      if (token !== previewToken) return;
      previewText.value = result.text;
      previewTruncated.value = result.truncated;
    }
  } catch (e) {
    if (token === previewToken) previewError.value = `${e}`;
  } finally {
    if (token === previewToken) previewBusy.value = false;
  }
}

function openSelected() {
  if (selected.value && previewKind.value === "document") {
    emit("open", selected.value.path);
  }
}

function onAssetDoubleClick(asset: AssetInfo) {
  if (isDocumentPath(asset.path)) emit("open", asset.path);
}

onUnmounted(clearPreview);

const KIND_LABELS: Record<string, string> = {
  all: "全部",
  model: "模型",
  texture: "贴图",
  env: "环境",
  scene: "场景",
  package: "包",
  data: "数据",
  other: "其他",
};

const kinds = computed(() => {
  const present = new Set((assets?.value ?? []).map((asset) => asset.kind));
  return [
    "all",
    ...Object.keys(KIND_LABELS).filter((key) => key !== "all" && present.has(key as AssetInfo["kind"])),
  ];
});

/**
 * Unity 风格过滤语法（`t:` 分类 / `l:` 标签 / `a:` 资产）。
 *
 * 命中时检索改由后端 `AssetDatabase.FindAssets`（`editor_find_assets`）处理，
 * 支持「多词 AND」与 `t:texture` 这类条件；否则走本地子串过滤，保持跟手。
 */
const FILTER_SYNTAX = /(^|\s)(t|l|a):/i;
/** 后端 `FindAssets` 的结果（`null` = 未启用后端检索）。 */
const backendMatches = ref<Set<string> | null>(null);

watch(
  () => search.value,
  async (value) => {
    const query = value.trim();
    if (!query || !FILTER_SYNTAX.test(query)) {
      backendMatches.value = null;
      return;
    }
    try {
      backendMatches.value = new Set(await api.findAssets(query));
    } catch {
      // 后端不可用（如纯浏览器预览）时退回本地过滤。
      backendMatches.value = null;
    }
  },
);

/** 命中检索/分类的全部资产（树模式用，不截断）。 */
const matched = computed(() => {
  const query = search.value.trim().toLowerCase();
  const all = assets?.value ?? [];
  const source = backendMatches.value
    ? all.filter((asset) => backendMatches.value?.has(asset.path))
    : all;
  return source
    .filter((asset) => kind.value === "all" || asset.kind === kind.value)
    .filter(
      (asset) =>
        // 后端已按 Unity 语法匹配过，不再叠加子串过滤。
        backendMatches.value !== null || !query || asset.path.toLowerCase().includes(query),
    );
});

/** 平铺列表只渲染前 400 项，避免一次创建过多 DOM。 */
const LIST_LIMIT = 400;
const filtered = computed(() => matched.value.slice(0, LIST_LIMIT));
const total = computed(() => (assets?.value ?? []).length);

// ---------------------------------------------------------------------------
// 目录树
// ---------------------------------------------------------------------------

interface TreeNode {
  /** 单段显示名。 */
  name: string;
  /** 相对 `assets/` 的目录或文件路径（`/` 分隔，根为 `""`）。 */
  path: string;
  dir: boolean;
  children: TreeNode[];
  asset?: AssetInfo;
  /** 子树里的文件数。 */
  count: number;
}

interface TreeRow {
  node: TreeNode;
  depth: number;
}

function buildTree(items: AssetInfo[], dirs: string[]): TreeNode {
  const root: TreeNode = { name: "assets", path: "", dir: true, children: [], count: 0 };

  // 先建目录（包括空目录），再挂文件；文件路径里的中间目录会复用已有节点。
  for (const dir of dirs) {
    const parts = dir.split("/").filter(Boolean);
    if (parts[0] === "assets") parts.shift();
    let node = root;
    for (const part of parts) {
      let child = node.children.find((c) => c.name === part && c.dir);
      if (!child) {
        child = {
          name: part,
          path: node.path ? `${node.path}/${part}` : part,
          dir: true,
          children: [],
          count: 0,
        };
        node.children.push(child);
      }
      node = child;
    }
  }

  for (const asset of items) {
    const parts = asset.path.split("/").filter(Boolean);
    // 后端返回的是相对项目根的路径；树根代表 `assets/`。
    if (parts[0] === "assets") parts.shift();
    if (!parts.length) continue;

    let node = root;
    parts.forEach((part, index) => {
      const isFile = index === parts.length - 1;
      let child = node.children.find((c) => c.name === part && c.dir !== isFile);
      if (!child) {
        child = {
          name: part,
          path: node.path ? `${node.path}/${part}` : part,
          dir: !isFile,
          children: [],
          count: 0,
          asset: isFile ? asset : undefined,
        };
        node.children.push(child);
      }
      if (!isFile) node = child;
    });
  }

  sortTree(root);
  countTree(root);
  return root;
}

/** 目录在前、文件在后，各自按名称排序。 */
function sortTree(node: TreeNode) {
  node.children.sort((a, b) => {
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    return a.name.localeCompare(b.name, "zh-CN");
  });
  node.children.forEach(sortTree);
}

function countTree(node: TreeNode): number {
  node.count = node.dir ? node.children.reduce((sum, child) => sum + countTree(child), 0) : 1;
  return node.count;
}

const tree = computed(() => buildTree(matched.value, assetDirs.value));

/** 展开状态（按相对 `assets/` 的目录路径记，根为 `""`）。 */
const expanded = reactive(new Set<string>([""]));
let treeInitialized = false;

watch(
  tree,
  (root) => {
    if (treeInitialized || root.children.length === 0) return;
    treeInitialized = true;
    // 默认展开第一层目录，避免打开面板只看到几个折叠项。
    for (const child of root.children) {
      if (child.dir) expanded.add(child.path);
    }
  },
  { immediate: true },
);

/** 检索/分类过滤时强制展开，保证匹配项可见。 */
const autoExpand = computed(() => search.value.trim() !== "" || kind.value !== "all");

function isOpen(node: TreeNode): boolean {
  return autoExpand.value || expanded.has(node.path);
}

function toggleDir(node: TreeNode) {
  if (autoExpand.value) return;
  if (expanded.has(node.path)) expanded.delete(node.path);
  else expanded.add(node.path);
}

/** 把树按当前展开状态压平成可见行，模板无需递归组件。 */
const treeRows = computed<TreeRow[]>(() => {
  const rows: TreeRow[] = [{ node: tree.value, depth: 0 }];
  if (!isOpen(tree.value)) return rows;
  const walk = (node: TreeNode, depth: number) => {
    for (const child of node.children) {
      rows.push({ node: child, depth });
      if (child.dir && isOpen(child)) walk(child, depth + 1);
    }
  };
  walk(tree.value, 1);
  return rows;
});

// ---------------------------------------------------------------------------
// 选择 / 引用 / 拖拽
// ---------------------------------------------------------------------------

async function select(asset: AssetInfo) {
  selected.value = asset;
  usage.value = [];
  usageError.value = "";
  busy.value = true;
  void loadPreview(asset);
  try {
    usage.value = await api.assetUsage(asset.path);
  } catch (e) {
    usageError.value = `${e}`;
  } finally {
    busy.value = false;
  }
}

function onRowClick(node: TreeNode) {
  selectedNode.value = { path: node.path, name: node.name, dir: node.dir };
  if (node.dir) {
    selected.value = null;
    usage.value = [];
    usageError.value = "";
    clearPreview();
    return;
  }
  if (node.asset) void select(node.asset);
}

function onRowDoubleClick(node: TreeNode) {
  if (node.dir) {
    toggleDir(node);
    return;
  }
  if (node.asset) onAssetDoubleClick(node.asset);
}

function selectAssetRow(asset: AssetInfo) {
  selectedNode.value = {
    path: asset.path.replace(/^assets\//, ""),
    name: asset.path.split("/").pop() ?? asset.path,
    dir: false,
  };
  void select(asset);
}

async function copyPath() {
  if (!selected.value) return;
  try {
    await navigator.clipboard.writeText(selected.value.path);
  } catch {
    // 剪贴板不可用时忽略（浏览器权限）。
  }
}

function openAssetMenu(node: TreeNode, event: MouseEvent) {
  onRowClick(node);
  const pos = clampMenuPosition(event.clientX, event.clientY, 190, 250);
  assetMenu.value = {
    path: node.path,
    name: node.name,
    dir: node.dir,
    ...pos,
  };
}

function openAssetMenuFromAsset(asset: AssetInfo, event: MouseEvent) {
  selectAssetRow(asset);
  const pos = clampMenuPosition(event.clientX, event.clientY, 190, 250);
  assetMenu.value = {
    path: asset.path.replace(/^assets\//, ""),
    name: asset.path.split("/").pop() ?? asset.path,
    dir: false,
    ...pos,
  };
}

function closeAssetMenu() {
  assetMenu.value = null;
}

async function copyAssetPath(path: string) {
  try {
    await navigator.clipboard.writeText(projectPath(path));
  } catch {
    // 剪贴板不可用时忽略（浏览器权限）。
  }
}

function runAssetMenu(
  action: "open" | "newdir" | "rename" | "delete" | "copy" | "duplicate" | "copyguid",
) {
  const menu = assetMenu.value;
  closeAssetMenu();
  if (!menu) return;
  if (action === "open") {
    openSelected();
    return;
  }
  if (action === "copy") {
    void copyAssetPath(menu.path);
    return;
  }
  if (action === "copyguid") {
    void copyAssetGuid(menu.path);
    return;
  }
  if (action === "duplicate") {
    void duplicateAsset(menu.path);
    return;
  }
  if (action === "newdir") {
    openNewDir();
    return;
  }
  if (action === "rename") {
    openRename();
    return;
  }
  void askDelete();
}

/**
 * 为「复制资产」(Unity `Ctrl+D`) 找一个不冲突的目标名：`a.gltf` → `a-2.gltf`。
 *
 * 与预制体实例化的去重后缀保持一致（见 `scene::instantiate_prefab`）。
 */
function uniqueCopyPath(path: string): string {
  const taken = new Set(
    (assets?.value ?? []).map((asset) => asset.path.replace(/^assets\//, "")),
  );
  const slash = path.lastIndexOf("/");
  const dir = slash >= 0 ? path.slice(0, slash + 1) : "";
  const file = path.slice(slash + 1);
  const dot = file.lastIndexOf(".");
  const stem = dot > 0 ? file.slice(0, dot) : file;
  const ext = dot > 0 ? file.slice(dot) : "";
  for (let n = 2; n < 1000; n += 1) {
    const candidate = `${dir}${stem}-${n}${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
  return `${dir}${stem}-copy${ext}`;
}

/** 复制资产 = `AssetDatabase.CopyAsset`（**不改写引用**，与 Unity 一致）。 */
async function duplicateAsset(path: string) {
  const target = uniqueCopyPath(path);
  await mutate(() => api.copyAsset(projectPath(path), projectPath(target)), {
    clearSelection: true,
  });
  if (!opError.value) selectedNode.value = { path: target, name: target.split("/").pop() ?? target, dir: false };
}

/** 复制资产 GUID = `AssetDatabase.AssetPathToGUID`。 */
async function copyAssetGuid(path: string) {
  try {
    const guid = await api.assetGuid(projectPath(path));
    await navigator.clipboard.writeText(guid);
  } catch {
    // 剪贴板不可用 / 路径非法时忽略（菜单已关闭）。
  }
}

const NODE_MIME = "application/x-prism-asset-node";

/** 拖拽到字段：自定义 MIME，避免和层级树的拖拽互相干扰。 */
function onDragStart(asset: AssetInfo, event: DragEvent) {
  event.dataTransfer?.setData("application/x-prism-asset", asset.path);
  event.dataTransfer?.setData("text/plain", asset.path);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}

function onNodeDragStart(node: TreeNode, event: DragEvent) {
  if (!node.path) {
    event.preventDefault();
    return;
  }
  event.dataTransfer?.setData(
    NODE_MIME,
    JSON.stringify({ path: node.path, dir: node.dir }),
  );
  if (!node.dir && node.asset) {
    event.dataTransfer?.setData("application/x-prism-asset", node.asset.path);
    event.dataTransfer?.setData("text/plain", node.asset.path);
  }
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}

const dragOverPath = ref<string | null>(null);
const ENTITY_MIME = "application/x-prism-entity";

function readDraggedEntity(event: DragEvent): { index: number; name: string } | null {
  const raw = event.dataTransfer?.getData(ENTITY_MIME);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as { index?: unknown; name?: unknown };
    if (typeof value.index !== "number") return null;
    return { index: value.index, name: typeof value.name === "string" ? value.name : "" };
  } catch {
    return null;
  }
}

function onFolderDragOver(node: TreeNode, event: DragEvent) {
  if (!node.dir) return;
  const entity = readDraggedEntity(event);
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = entity ? "copy" : "move";
  dragOverPath.value = node.path;
}

function readDraggedNode(event: DragEvent): { path: string; dir: boolean } | null {
  const raw = event.dataTransfer?.getData(NODE_MIME);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as { path?: unknown; dir?: unknown };
    if (typeof value.path !== "string" || typeof value.dir !== "boolean") return null;
    return { path: value.path, dir: value.dir };
  } catch {
    return null;
  }
}

function parentOf(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? "" : path.slice(0, index);
}

function baseName(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? path : path.slice(index + 1);
}

function joinRel(base: string, name: string): string {
  return base ? `${base}/${name}` : name;
}

/** 树内相对路径 -> 项目相对路径（`assets/...`）。 */
function projectPath(rel: string): string {
  return rel ? `assets/${rel}` : "assets";
}

async function onDropFolder(target: TreeNode, event: DragEvent) {
  dragOverPath.value = null;
  if (!target.dir) return;
  const entity = readDraggedEntity(event);
  if (entity) {
    emit("prefab-drop", projectPath(target.path), entity.index, entity.name);
    return;
  }

  const dragged = readDraggedNode(event);
  if (!dragged || !dragged.path || !target.dir) return;
  if (dragged.path === target.path) return;
  if (dragged.dir && target.path.startsWith(`${dragged.path}/`)) return;
  if (parentOf(dragged.path) === target.path) return;

  const to = joinRel(target.path, baseName(dragged.path));
  await mutate(
    () => api.moveAsset(projectPath(dragged.path), projectPath(to)),
    { clearSelection: true },
  );
}

// ---------------------------------------------------------------------------
// 文件操作
// ---------------------------------------------------------------------------

interface NameDialog {
  kind: "newdir" | "rename";
  title: string;
  /** 相对 `assets/` 的父目录。 */
  base: string;
  value: string;
  /** 重命名时的原项目路径。 */
  from?: string;
}

const dialog = ref<NameDialog | null>(null);
const deleteTarget = ref<{ path: string; name: string; refs: string[] } | null>(null);

function openNewDir() {
  opError.value = "";
  const node = selectedNode.value;
  const base = node ? (node.dir ? node.path : parentOf(node.path)) : "";
  dialog.value = {
    kind: "newdir",
    title: `在 ${projectPath(base)} 下新建文件夹`,
    base,
    value: "",
  };
}

function openRename() {
  const node = selectedNode.value;
  if (!node || !node.path) return;
  opError.value = "";
  dialog.value = {
    kind: "rename",
    title: `重命名 / 移动 ${projectPath(node.path)}`,
    base: parentOf(node.path),
    value: node.name,
    from: projectPath(node.path),
  };
}

async function submitDialog() {
  const current = dialog.value;
  if (!current || opBusy.value) return;
  const name = current.value.trim();
  if (!name) {
    opError.value = "名称不能为空";
    return;
  }
  if (name.includes("/") || name.includes("\\")) {
    opError.value = "名称不能包含斜杠（移动请直接拖拽）";
    return;
  }
  const rel = joinRel(current.base, name);

  opBusy.value = true;
  opError.value = "";
  try {
    if (current.kind === "newdir") {
      await api.createAssetDir(projectPath(rel));
      dialog.value = null;
      expanded.add(current.base);
      await refreshAssets();
      selectedNode.value = { path: rel, name, dir: true };
      selected.value = null;
    } else {
      if (!(await ensureSaved())) return;
      const result = await api.renameAsset(current.from ?? projectPath(rel), projectPath(rel));
      dialog.value = null;
      await refreshAssets();
      selectedNode.value = { path: rel, name, dir: false };
      const next = (assets?.value ?? []).find((asset) => asset.path === projectPath(rel));
      if (next) await select(next);
      notify(result);
    }
  } catch (e) {
    opError.value = `${e}`;
  } finally {
    opBusy.value = false;
  }
}

async function askDelete() {
  const node = selectedNode.value;
  if (!node || !node.path || opBusy.value) return;
  opBusy.value = true;
  opError.value = "";
  try {
    const path = projectPath(node.path);
    const refs = await api.assetReferences(path);
    deleteTarget.value = { path, name: node.name, refs };
  } catch (e) {
    opError.value = `${e}`;
  } finally {
    opBusy.value = false;
  }
}

async function confirmDelete() {
  const target = deleteTarget.value;
  if (!target || opBusy.value) return;
  if (!(await ensureSaved())) return;
  opBusy.value = true;
  try {
    const result = await api.deleteAsset(target.path, true);
    deleteTarget.value = null;
    selectedNode.value = null;
    selected.value = null;
    usage.value = [];
    clearPreview();
    await refreshAssets();
    notify(result);
  } catch (e) {
    opError.value = `${e}`;
  } finally {
    opBusy.value = false;
  }
}

async function ensureSaved(): Promise<boolean> {
  if (!props.beforeChange) return true;
  return props.beforeChange();
}

async function mutate(
  operation: () => Promise<AssetMutation>,
  options: { clearSelection?: boolean } = {},
) {
  if (opBusy.value) return;
  if (!(await ensureSaved())) return;
  opBusy.value = true;
  opError.value = "";
  try {
    const result = await operation();
    if (options.clearSelection) {
      selectedNode.value = null;
      selected.value = null;
      usage.value = [];
      clearPreview();
    }
    await refreshAssets();
    notify(result);
  } catch (e) {
    opError.value = `${e}`;
  } finally {
    opBusy.value = false;
  }
}

async function refreshAssets() {
  const [list, dirs] = await Promise.all([
    api.listAssetsInfo().catch(() => assets?.value ?? []),
    api.listAssetDirs().catch(() => assetDirs.value),
  ]);
  if (assets) assets.value = list;
  assetDirs.value = dirs;
}

function notify(result: AssetMutation) {
  opError.value = "";
  emit("changed", result.affectedDocuments);
}

onMounted(() => {
  window.addEventListener("pointerdown", closeAssetMenu);
  window.addEventListener("scroll", closeAssetMenu, true);
  window.addEventListener("blur", closeAssetMenu);
  window.addEventListener("resize", closeAssetMenu);
  void refreshAssets();
});

onUnmounted(() => {
  window.removeEventListener("pointerdown", closeAssetMenu);
  window.removeEventListener("scroll", closeAssetMenu, true);
  window.removeEventListener("blur", closeAssetMenu);
  window.removeEventListener("resize", closeAssetMenu);
});
</script>

<template>
  <div class="asset-browser">
    <div class="tree-tools">
    <div class="asset-filter-row">
      <button
        class="ghost icon asset-new-dir"
        :disabled="opBusy"
        title="在所选目录下新建文件夹"
        @click="openNewDir"
      >
        <Plus :size="14" />
      </button>
      <input v-model="search" type="text" :placeholder="`检索 ${total} 个资产…`" />
      <select v-model="kind" title="按资产类型过滤">
        <option v-for="k in kinds" :key="k" :value="k">{{ KIND_LABELS[k] ?? k }}</option>
      </select>
    </div>
    <div v-if="opError" class="error asset-op-error" :title="opError">{{ opError }}</div>

    <div v-if="dialog" class="asset-dialog">
      <div class="hint">{{ dialog.title }}</div>
      <input
        v-model="dialog.value"
        type="text"
        autofocus
        @keydown.enter.prevent="submitDialog"
        @keydown.esc.prevent="dialog = null"
      />
      <div class="asset-dialog-actions">
        <button class="ghost" @click="dialog = null">取消</button>
        <button class="primary" :disabled="opBusy || !dialog.value.trim()" @click="submitDialog">
          确定
        </button>
      </div>
    </div>

    <div v-if="deleteTarget" class="asset-dialog">
      <div class="hint">删除 {{ deleteTarget.path }}？</div>
      <div v-if="deleteTarget.refs.length" class="asset-warning">
        仍被 {{ deleteTarget.refs.length }} 个文档引用：
        <div v-for="ref in deleteTarget.refs.slice(0, 5)" :key="ref" class="asset-warning-ref">
          {{ ref }}
        </div>
        <div v-if="deleteTarget.refs.length > 5" class="asset-warning-ref">
          … 还有 {{ deleteTarget.refs.length - 5 }} 个
        </div>
      </div>
      <div class="asset-dialog-actions">
        <button class="ghost" @click="deleteTarget = null">取消</button>
        <button class="primary" :disabled="opBusy" @click="confirmDelete">确认删除</button>
      </div>
    </div>
    </div>

    <!-- 目录树 / 列表：只让这一块滚动，工具栏和详情固定 -->
    <div class="asset-scroll">
  <template v-if="currentView === 'tree'">
    <div
      v-for="row in treeRows"
      :key="row.node.path || 'root'"
      class="list-item asset-row"
      :class="{
        active:
          selectedNode?.path === row.node.path && selectedNode?.dir === row.node.dir,
        'drop-target': dragOverPath === row.node.path && row.node.dir,
      }"
      :style="{ paddingLeft: `${6 + row.depth * 12}px` }"
      :draggable="!!row.node.path"
      :title="row.node.dir ? row.node.path || 'assets' : `可拖到路径字段：${row.node.path}`"
      @dragstart="onNodeDragStart(row.node, $event)"
      @dragover="onFolderDragOver(row.node, $event)"
      @dragleave="dragOverPath = null"
      @drop.stop.prevent="onDropFolder(row.node, $event)"
      @click="onRowClick(row.node)"
      @dblclick="onRowDoubleClick(row.node)"
      @contextmenu.prevent.stop="openAssetMenu(row.node, $event)"
    >
      <span
        v-if="row.node.dir"
        class="folder-caret"
        @click.stop="toggleDir(row.node)"
      >
        <ChevronDown v-if="isOpen(row.node)" :size="12" />
        <ChevronRight v-else :size="12" />
      </span>
      <span v-else class="badge">{{ row.node.asset?.kind }}</span>
      <span class="asset-name">{{ row.node.name }}</span>
      <span v-if="row.node.dir" class="muted">{{ row.node.count }}</span>
      <span v-else class="muted">{{ formatBytes(row.node.asset?.sizeBytes ?? 0) }}</span>
    </div>
    <div v-if="!treeRows.length" class="hint" style="padding: 8px">没有匹配的资产</div>
  </template>

  <!-- 平铺列表 -->
  <template v-else>
    <div
      v-for="asset in filtered"
      :key="asset.path"
      class="list-item"
      :class="{ active: asset.path === selected?.path }"
      draggable="true"
      title="可拖到路径字段"
      @dragstart="onDragStart(asset, $event)"
      @click="selectAssetRow(asset)"
      @dblclick="onAssetDoubleClick(asset)"
      @contextmenu.prevent.stop="openAssetMenuFromAsset(asset, $event)"
    >
      <span class="badge">{{ asset.kind }}</span>
      <span class="path">{{ asset.path }}</span>
      <span class="muted">{{ formatBytes(asset.sizeBytes) }}</span>
    </div>
    <div v-if="!filtered.length" class="hint" style="padding: 8px">没有匹配的资产</div>
    <div v-else-if="matched.length > LIST_LIMIT" class="hint" style="padding: 8px">
      共 {{ matched.length }} 项，仅显示前 {{ LIST_LIMIT }} 项；继续输入以缩小范围，或切到「目录」。
    </div>
  </template>
    </div>

  <div v-if="selected" class="asset-detail">
    <div class="hint path">{{ selected.path }}</div>
    <div class="hint" title="Unity AssetImporter.GetAtPath(...).GetType().Name">
      {{ selected.kind }} · 导入器 {{ selected.importer }}
    </div>

    <div v-if="previewKind === 'document'" class="asset-preview-actions">
      <button class="primary" @click="openSelected">打开文档</button>
    </div>
    <div v-else-if="previewBusy" class="hint">加载预览…</div>
    <div v-else-if="previewError" class="hint error">{{ previewError }}</div>
    <template v-else-if="previewKind === 'image' && previewUrl">
      <img class="asset-preview-image" :src="previewUrl" :alt="selected.path" />
    </template>
    <template v-else-if="previewKind === 'text'">
      <pre class="asset-preview-text">{{ previewText }}</pre>
      <div v-if="previewTruncated" class="asset-preview-note">已截断，仅显示前 512 KB</div>
    </template>

    <div class="hint">
      引用：<template v-if="busy">…</template>
      <template v-else-if="usageError">{{ usageError }}</template>
      <template v-else-if="usage.length">{{ usage.length }} 处</template>
      <template v-else>未被任何场景 / 预制体 / 构建清单引用</template>
    </div>
    <button
      v-for="(ref, i) in usage"
      :key="i"
      class="usage-link"
      :title="
        ref.kind === 'manifest'
          ? '构建清单（assets/scenes.toml）里的场景条目'
          : '打开该文档并定位到这个字段'
      "
      @click="emit('jump', ref)"
    >
      <template v-if="ref.kind === 'manifest'">
        {{ ref.scene }} · {{ ref.entity }}
      </template>
      <template v-else>
        {{ ref.scene }} · #{{ ref.entityIndex }} {{ ref.entity }} ·
        {{ ref.component.split("::").pop() }}.{{ ref.field }}
      </template>
    </button>
    <button class="ghost icon" style="margin-top: 4px" @click="copyPath">复制路径</button>
  </div>
  </div>

  <Teleport to="body">
    <div
      v-if="assetMenu"
      class="entity-context-menu asset-context-menu"
      :style="{ left: `${assetMenu.x}px`, top: `${assetMenu.y}px` }"
      @pointerdown.stop
      @click.stop
      @contextmenu.prevent
    >
      <button
        v-if="!assetMenu.dir && isDocumentPath(projectPath(assetMenu.path))"
        @click="runAssetMenu('open')"
      >
        <span>打开文档</span>
      </button>
      <button v-if="assetMenu.dir" @click="runAssetMenu('newdir')">
        <span>新建文件夹</span>
      </button>
      <button v-if="assetMenu.path" :disabled="opBusy" @click="runAssetMenu('rename')">
        <span>重命名 / 移动</span>
      </button>
      <button v-if="assetMenu.path && !assetMenu.dir" :disabled="opBusy" @click="runAssetMenu('duplicate')">
        <span>复制资产</span>
      </button>
      <button @click="runAssetMenu('copy')">
        <span>复制路径</span>
      </button>
      <button v-if="assetMenu.path && !assetMenu.dir" @click="runAssetMenu('copyguid')">
        <span>复制 GUID</span>
      </button>
      <button v-if="assetMenu.path" class="danger" :disabled="opBusy" @click="runAssetMenu('delete')">
        <span>删除</span>
      </button>
    </div>
  </Teleport>
</template>
