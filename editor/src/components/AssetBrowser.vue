<script setup lang="ts">
import { computed, inject, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import {
  Box,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  File,
  FileCode,
  FileText,
  Folder,
  FolderOpen,
  Image as ImageIcon,
  Music,
  Plus,
  RefreshCw,
  Search,
  Sparkles,
  X,
} from "lucide-vue-next";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";
import * as api from "../api";
import { ASSETS_KEY } from "../keys";
import { formatBytes } from "../format";
import { clampMenuPosition } from "../contextMenu";
import { t } from "../i18n";
import type { AssetInfo, AssetMutation, AssetRef } from "../types";

/**
 * 资产管理器面板（7 Project Browser 标准双栏设计）：
 * - 左侧：文件夹树（仅展示目录层级，支持展开/折叠、点击切换当前目录、拖拽移动与预制体保存）。
 * - 竖向 Splitter：可平滑拖拽调整左右栏宽度，记忆到 localStorage。
 * - 顶部工具栏：后退/前进导航历史、面包屑导航（点击快速跳转上级）、搜索过滤与分类下拉。
 * - 右侧：文件夹内容区（ListArea），展示当前目录下的子文件夹与资产。
 * - 底部状态栏：显示项目计数或选中资产信息，以及 常用标准图标大小滑块（16px 为列表视图，>16px 为网格视图）。
 * - 自定义 Editor UI 对话框与右键菜单，禁止使用原生 window.confirm/prompt。
 */
const props = defineProps<{
  /** 写操作前确保当前文档已保存（由 App 注入）。 */
  beforeChange?: () => Promise<boolean>;
}>();

const emit = defineEmits<{
  (e: "select", asset: AssetInfo | null): void;
  (e: "jump", ref: AssetRef): void;
  (e: "changed", affected: string[]): void;
  /** 双击场景/预制体：请求宿主打开对应文档。 */
  (e: "open", path: string): void;
  /** 从层级树拖入资产目录：把实体子树保存为预制体。 */
  (e: "prefab-drop", folder: string, entityIndex: number, name: string): void;
}>();

const assets = inject(ASSETS_KEY, null);
const assetDirs = ref<string[]>([]);

// ---------------------------------------------------------------------------
// 布局与尺寸持久化
// ---------------------------------------------------------------------------

const TREE_WIDTH_STORAGE_KEY = "prisma-editor.project-browser.tree-width";
const ICON_SIZE_STORAGE_KEY = "prisma-editor.project-browser.icon-size";

function getInitialTreeWidth(): number {
  try {
    const val = Number(localStorage.getItem(TREE_WIDTH_STORAGE_KEY));
    if (!isNaN(val) && val >= 100 && val <= 400) return val;
  } catch {
    // Local storage unavailable
  }
  return 180;
}

function getInitialIconSize(): number {
  try {
    const val = Number(localStorage.getItem(ICON_SIZE_STORAGE_KEY));
    if (!isNaN(val) && val >= 16 && val <= 96) return val;
  } catch {
    // Local storage unavailable
  }
  return 16;
}

const treeWidth = ref(getInitialTreeWidth());
const iconSize = ref(getInitialIconSize());

watch(treeWidth, (val) => {
  try {
    localStorage.setItem(TREE_WIDTH_STORAGE_KEY, String(val));
  } catch {}
});

watch(iconSize, (val) => {
  try {
    localStorage.setItem(ICON_SIZE_STORAGE_KEY, String(val));
  } catch {}
});

// ---------------------------------------------------------------------------
// 检索与分类过滤
// ---------------------------------------------------------------------------

const search = ref("");
const kind = ref("all");

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
    ...Object.keys(KIND_LABELS).filter(
      (key) => key !== "all" && present.has(key as AssetInfo["kind"]),
    ),
  ];
});

const kindDropdownOptions = computed<DropdownOption[]>(() =>
  kinds.value.map((k) => ({ value: k, label: KIND_LABELS[k] ?? k })),
);

const isSearching = computed(
  () => search.value.trim().length > 0 || kind.value !== "all",
);

const FILTER_SYNTAX = /(^|\s)(t|l|a):/i;
const backendMatches = ref<Set<string> | null>(null);
let searchToken = 0;

watch(
  () => search.value,
  async (value) => {
    const token = ++searchToken;
    const query = value.trim();
    if (!query || !FILTER_SYNTAX.test(query)) {
      backendMatches.value = null;
      return;
    }
    try {
      const matches = await api.findAssets(query);
      if (token !== searchToken) return;
      backendMatches.value = new Set(matches);
    } catch {
      if (token !== searchToken) return;
      backendMatches.value = null;
    }
  },
);

const matchedAssets = computed(() => {
  const query = search.value.trim().toLowerCase();
  const all = assets?.value ?? [];
  const source = backendMatches.value
    ? all.filter((asset) => backendMatches.value?.has(asset.path))
    : all;
  return source
    .filter((asset) => kind.value === "all" || asset.kind === kind.value)
    .filter(
      (asset) =>
        backendMatches.value !== null ||
        !query ||
        asset.path.toLowerCase().includes(query),
    );
});

// ---------------------------------------------------------------------------
// 目录树模型（左侧目录面板）
// ---------------------------------------------------------------------------

interface FolderNode {
  name: string;
  relPath: string; // "" 为根 Assets，"scenes", "textures/nature"
  fullPath: string; // "assets", "assets/scenes"
  subfolders: FolderNode[];
  assetCount: number;
}

interface VisibleFolderRow {
  name: string;
  relPath: string;
  depth: number;
  hasChildren: boolean;
  assetCount: number;
}

const expandedFolders = reactive(new Set<string>([""]));
let initialExpandedDone = false;

function buildFolderTree(): FolderNode {
  const root: FolderNode = {
    name: "Assets",
    relPath: "",
    fullPath: "assets",
    subfolders: [],
    assetCount: 0,
  };

  const registeredDirs = new Set<string>();

  // 1. 收集明确列出的目录
  for (const dir of assetDirs.value) {
    const clean = dir.replace(/^assets\/?/, "").replace(/\/+$/, "");
    if (clean) registeredDirs.add(clean);
  }

  // 2. 从资产文件路径中提取中间目录
  for (const asset of assets?.value ?? []) {
    const clean = asset.path.replace(/^assets\/?/, "");
    const slash = clean.lastIndexOf("/");
    if (slash > 0) {
      let cur = "";
      for (const seg of clean.slice(0, slash).split("/")) {
        cur = cur ? `${cur}/${seg}` : seg;
        registeredDirs.add(cur);
      }
    }
  }

  // 3. 构建多层级目录树
  for (const dir of Array.from(registeredDirs).sort()) {
    const parts = dir.split("/").filter(Boolean);
    let cur = root;
    let accumulated = "";
    for (const part of parts) {
      accumulated = accumulated ? `${accumulated}/${part}` : part;
      let next = cur.subfolders.find((f) => f.name === part);
      if (!next) {
        next = {
          name: part,
          relPath: accumulated,
          fullPath: `assets/${accumulated}`,
          subfolders: [],
          assetCount: 0,
        };
        cur.subfolders.push(next);
      }
      cur = next;
    }
  }

  // 4. 计算每个目录包含的资产数量
  for (const asset of assets?.value ?? []) {
    const clean = asset.path.replace(/^assets\/?/, "");
    const slash = clean.lastIndexOf("/");
    const parentDir = slash > 0 ? clean.slice(0, slash) : "";
    let cur = root;
    cur.assetCount += 1;
    if (parentDir) {
      for (const seg of parentDir.split("/")) {
        const next = cur.subfolders.find((f) => f.name === seg);
        if (next) {
          next.assetCount += 1;
          cur = next;
        } else break;
      }
    }
  }

  function sortFolders(node: FolderNode) {
    node.subfolders.sort((a, b) => a.name.localeCompare(b.name, "zh-CN"));
    node.subfolders.forEach(sortFolders);
  }
  sortFolders(root);

  return root;
}

const folderTree = computed(() => buildFolderTree());

watch(
  folderTree,
  (root) => {
    if (initialExpandedDone || root.subfolders.length === 0) return;
    initialExpandedDone = true;
    for (const child of root.subfolders) {
      expandedFolders.add(child.relPath);
    }
  },
  { immediate: true },
);

const visibleFolderRows = computed<VisibleFolderRow[]>(() => {
  const rows: VisibleFolderRow[] = [];
  function traverse(node: FolderNode, depth: number) {
    rows.push({
      name: node.name,
      relPath: node.relPath,
      depth,
      hasChildren: node.subfolders.length > 0,
      assetCount: node.assetCount,
    });
    if (expandedFolders.has(node.relPath)) {
      for (const child of node.subfolders) {
        traverse(child, depth + 1);
      }
    }
  }
  traverse(folderTree.value, 0);
  return rows;
});

function isFolderExpanded(relPath: string): boolean {
  return expandedFolders.has(relPath);
}

function toggleFolderExpanded(relPath: string) {
  if (expandedFolders.has(relPath)) {
    expandedFolders.delete(relPath);
  } else {
    expandedFolders.add(relPath);
  }
}

// ---------------------------------------------------------------------------
// 导航历史与当前激活目录
// ---------------------------------------------------------------------------

const activeFolder = ref<string>("");
const navHistory = ref<string[]>([""]);
const navHistoryIndex = ref(0);

function navigateTo(relPath: string) {
  if (relPath === activeFolder.value) return;
  navHistory.value = navHistory.value.slice(0, navHistoryIndex.value + 1);
  navHistory.value.push(relPath);
  navHistoryIndex.value = navHistory.value.length - 1;
  activeFolder.value = relPath;

  // 展开该路径的所有祖先节点
  let p = "";
  for (const seg of relPath.split("/").filter(Boolean)) {
    p = p ? `${p}/${seg}` : seg;
    expandedFolders.add(p);
  }
}

function goBack() {
  if (navHistoryIndex.value > 0) {
    navHistoryIndex.value -= 1;
    activeFolder.value = navHistory.value[navHistoryIndex.value];
  }
}

function goForward() {
  if (navHistoryIndex.value < navHistory.value.length - 1) {
    navHistoryIndex.value += 1;
    activeFolder.value = navHistory.value[navHistoryIndex.value];
  }
}

const breadcrumbs = computed(() => {
  const list = [{ name: "Assets", relPath: "" }];
  if (!activeFolder.value) return list;
  const parts = activeFolder.value.split("/").filter(Boolean);
  let cur = "";
  for (const part of parts) {
    cur = cur ? `${cur}/${part}` : part;
    list.push({ name: part, relPath: cur });
  }
  return list;
});

// ---------------------------------------------------------------------------
// 右侧内容区（ListArea 条目）
// ---------------------------------------------------------------------------

export interface ContentEntry {
  id: string;
  name: string;
  relPath: string;
  fullPath: string;
  isDir: boolean;
  asset?: AssetInfo;
  sizeBytes?: number;
  kind?: string;
}

const currentEntries = computed<ContentEntry[]>(() => {
  if (isSearching.value) {
    // 搜索模式：直接展示所有匹配资产
    return matchedAssets.value.map((asset) => {
      const clean = asset.path.replace(/^assets\/?/, "");
      const name = clean.split("/").pop() ?? clean;
      return {
        id: asset.path,
        name,
        relPath: clean,
        fullPath: asset.path,
        isDir: false,
        asset,
        sizeBytes: asset.sizeBytes,
        kind: asset.kind,
      };
    });
  }

  // 正常浏览模式：展示当前目录下的直接子文件夹与直接资产
  const entries: ContentEntry[] = [];
  const currentRel = activeFolder.value;

  // 1. 查找当前目录的子文件夹
  let targetNode = folderTree.value;
  if (currentRel) {
    const parts = currentRel.split("/").filter(Boolean);
    for (const part of parts) {
      const next = targetNode.subfolders.find((f) => f.name === part);
      if (next) targetNode = next;
      else break;
    }
  }

  for (const sub of targetNode.subfolders) {
    entries.push({
      id: `dir:${sub.relPath}`,
      name: sub.name,
      relPath: sub.relPath,
      fullPath: sub.fullPath,
      isDir: true,
      sizeBytes: undefined,
      kind: undefined,
    });
  }

  // 2. 查找直接位于当前目录下的资产文件
  for (const asset of assets?.value ?? []) {
    const clean = asset.path.replace(/^assets\/?/, "");
    const slash = clean.lastIndexOf("/");
    const parent = slash > 0 ? clean.slice(0, slash) : "";
    if (parent === currentRel) {
      const name = slash >= 0 ? clean.slice(slash + 1) : clean;
      entries.push({
        id: asset.path,
        name,
        relPath: clean,
        fullPath: asset.path,
        isDir: false,
        asset,
        sizeBytes: asset.sizeBytes,
        kind: asset.kind,
      });
    }
  }

  return entries;
});

// ---------------------------------------------------------------------------
// 选中与详情状态
// ---------------------------------------------------------------------------

const selectedEntry = ref<ContentEntry | null>(null);

function selectEntry(entry: ContentEntry | null) {
  selectedEntry.value = entry;
  if (entry && !entry.isDir && entry.asset) {
    emit("select", entry.asset);
  } else {
    emit("select", null);
  }
}

function onFolderRowClick(relPath: string) {
  navigateTo(relPath);
  selectEntry(null);
}

function onEntryClick(entry: ContentEntry) {
  selectEntry(entry);
}

function onEntryDoubleClick(entry: ContentEntry) {
  if (entry.isDir) {
    navigateTo(entry.relPath);
    selectEntry(null);
  } else if (entry.asset && isDocumentPath(entry.fullPath)) {
    emit("open", entry.fullPath);
  }
}

function isDocumentPath(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.endsWith(".scene.json") || lower.endsWith(".prefab.json");
}

function getEntryIcon(entry: ContentEntry) {
  if (entry.isDir) {
    return activeFolder.value === entry.relPath ? FolderOpen : Folder;
  }
  const ext = entry.name.split(".").pop()?.toLowerCase() ?? "";
  if (entry.kind === "scene" || entry.name.endsWith(".scene.json")) return Sparkles;
  if (entry.kind === "package" || entry.name.endsWith(".prefab.json")) return Box;
  if (entry.kind === "model" || ["fbx", "gltf", "glb", "obj"].includes(ext)) return Box;
  if (entry.kind === "texture" || ["png", "jpg", "jpeg", "webp", "hdr"].includes(ext)) return ImageIcon;
  if (["slang", "wgsl", "glsl", "hlsl", "shader"].includes(ext)) return FileCode;
  if (["wav", "ogg", "mp3"].includes(ext)) return Music;
  if (["json", "toml", "txt", "md"].includes(ext)) return FileText;
  return File;
}

function getEntryIconClass(entry: ContentEntry): string {
  if (entry.isDir) return "icon-folder";
  if (entry.kind === "scene" || entry.name.endsWith(".scene.json")) return "icon-scene";
  if (entry.kind === "package" || entry.name.endsWith(".prefab.json")) return "icon-prefab";
  if (entry.kind === "model") return "icon-model";
  if (entry.kind === "texture") return "icon-texture";
  if (entry.kind === "data") return "icon-data";
  return "icon-generic";
}

const statusLabel = computed(() => {
  if (selectedEntry.value) {
    const entry = selectedEntry.value;
    if (entry.isDir) {
      return `${entry.fullPath}/ (${t("assets.folder")})`;
    }
    return `${entry.fullPath} (${formatBytes(entry.sizeBytes ?? 0)})`;
  }
  const count = currentEntries.value.length;
  return t(count === 1 ? "assets.itemCountSingle" : "assets.itemsCount", { count });
});

// ---------------------------------------------------------------------------
// 拖拽与移动
// ---------------------------------------------------------------------------

const NODE_MIME = "application/x-prism-asset-node";
const ENTITY_MIME = "application/x-prism-entity";
const dragOverFolder = ref<string | null>(null);

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

function onFolderDragStart(folder: { relPath: string }, event: DragEvent) {
  if (!folder.relPath) {
    event.preventDefault();
    return;
  }
  event.dataTransfer?.setData(
    NODE_MIME,
    JSON.stringify({ path: folder.relPath, dir: true }),
  );
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}

function onEntryDragStart(entry: ContentEntry, event: DragEvent) {
  event.dataTransfer?.setData(
    NODE_MIME,
    JSON.stringify({ path: entry.relPath, dir: entry.isDir }),
  );
  if (!entry.isDir) {
    event.dataTransfer?.setData("application/x-prism-asset", entry.fullPath);
    event.dataTransfer?.setData("text/plain", entry.fullPath);
  }
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}

function onFolderDragOver(targetRelPath: string, event: DragEvent) {
  event.preventDefault();
  const entity = readDraggedEntity(event);
  if (event.dataTransfer) event.dataTransfer.dropEffect = entity ? "copy" : "move";
  dragOverFolder.value = targetRelPath;
}

function onFolderDragLeave() {
  dragOverFolder.value = null;
}

async function onFolderDrop(targetRelPath: string, event: DragEvent) {
  dragOverFolder.value = null;
  const targetFullPath = targetRelPath ? `assets/${targetRelPath}` : "assets";

  // 1. 处理从场景层级拖拽实体 -> 保存为预制体
  const entity = readDraggedEntity(event);
  if (entity) {
    emit("prefab-drop", targetFullPath, entity.index, entity.name);
    return;
  }

  // 2. 处理项目浏览器内资产/目录移动
  const dragged = readDraggedNode(event);
  if (!dragged || !dragged.path) return;
  if (dragged.path === targetRelPath) return;
  if (dragged.dir && targetRelPath.startsWith(`${dragged.path}/`)) return;

  const fileName = dragged.path.split("/").pop() ?? dragged.path;
  const destination = targetRelPath ? `${targetRelPath}/${fileName}` : fileName;
  if (dragged.path === destination) return;

  await mutate(
    () => api.moveAsset(`assets/${dragged.path}`, `assets/${destination}`),
    { clearSelection: true },
  );
}

function onContentsDragOver(event: DragEvent) {
  onFolderDragOver(activeFolder.value, event);
}

function onContentsDragLeave() {
  onFolderDragLeave();
}

async function onContentsDrop(event: DragEvent) {
  await onFolderDrop(activeFolder.value, event);
}

// ---------------------------------------------------------------------------
// 垂直 Splitter 拖拽调整大小
// ---------------------------------------------------------------------------

const browserRoot = ref<HTMLElement | null>(null);
let isResizingSplitter = false;

function startSplitterResize(event: PointerEvent) {
  isResizingSplitter = true;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);

  const onPointerMove = (e: PointerEvent) => {
    if (!isResizingSplitter || !browserRoot.value) return;
    const rect = browserRoot.value.getBoundingClientRect();
    const newWidth = Math.max(110, Math.min(360, e.clientX - rect.left));
    treeWidth.value = newWidth;
  };

  const onPointerUp = (e: PointerEvent) => {
    isResizingSplitter = false;
    try {
      target.releasePointerCapture(e.pointerId);
    } catch {}
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerUp);
  };

  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);
}

// ---------------------------------------------------------------------------
// 右键上下文菜单
// ---------------------------------------------------------------------------

interface ContextMenuState {
  x: number;
  y: number;
  isItem: boolean;
  isDir: boolean;
  name: string;
  relPath: string;
  fullPath: string;
}

const contextMenu = ref<ContextMenuState | null>(null);

function openFolderContextMenu(folder: { name: string; relPath: string }, event: MouseEvent) {
  navigateTo(folder.relPath);
  selectEntry(null);
  const pos = clampMenuPosition(event.clientX, event.clientY, 180, 240);
  contextMenu.value = {
    x: pos.x,
    y: pos.y,
    isItem: true,
    isDir: true,
    name: folder.name,
    relPath: folder.relPath,
    fullPath: folder.relPath ? `assets/${folder.relPath}` : "assets",
  };
}

function openEntryContextMenu(entry: ContentEntry, event: MouseEvent) {
  selectEntry(entry);
  const pos = clampMenuPosition(event.clientX, event.clientY, 180, 240);
  contextMenu.value = {
    x: pos.x,
    y: pos.y,
    isItem: true,
    isDir: entry.isDir,
    name: entry.name,
    relPath: entry.relPath,
    fullPath: entry.fullPath,
  };
}

function openEmptyAreaContextMenu(event: MouseEvent) {
  selectEntry(null);
  const pos = clampMenuPosition(event.clientX, event.clientY, 180, 200);
  contextMenu.value = {
    x: pos.x,
    y: pos.y,
    isItem: false,
    isDir: true,
    name: activeFolder.value.split("/").pop() ?? "Assets",
    relPath: activeFolder.value,
    fullPath: activeFolder.value ? `assets/${activeFolder.value}` : "assets",
  };
}

function closeContextMenu() {
  contextMenu.value = null;
}

function onEmptyAreaClick() {
  selectEntry(null);
  closeContextMenu();
}

async function copyPath(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {}
}

async function copyAssetGuid(path: string) {
  try {
    const guid = await api.assetGuid(path);
    await navigator.clipboard.writeText(guid);
  } catch {}
}

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

async function duplicateAsset(relPath: string) {
  const target = uniqueCopyPath(relPath);
  await mutate(
    () => api.copyAsset(`assets/${relPath}`, `assets/${target}`),
    { clearSelection: true },
  );
}

// ---------------------------------------------------------------------------
// 自定义 Editor UI 对话框（新建文件夹 / 重命名 / 删除确认）
// ---------------------------------------------------------------------------

interface DialogState {
  kind: "newdir" | "rename";
  title: string;
  baseRel: string;
  value: string;
  fromFullPath?: string;
}

const dialog = ref<DialogState | null>(null);
const deleteTarget = ref<{ path: string; name: string; refs: string[] } | null>(null);
const opBusy = ref(false);
const opError = ref("");

function openNewDir(baseRel: string) {
  opError.value = "";
  const baseFull = baseRel ? `assets/${baseRel}` : "assets";
  dialog.value = {
    kind: "newdir",
    title: t("assets.createInFolder", { folder: baseFull }),
    baseRel,
    value: "",
  };
}

function openRename(entry: { name: string; relPath: string; fullPath: string }) {
  opError.value = "";
  const slash = entry.relPath.lastIndexOf("/");
  const baseRel = slash > 0 ? entry.relPath.slice(0, slash) : "";
  dialog.value = {
    kind: "rename",
    title: t("assets.renameAsset", { name: entry.name }),
    baseRel,
    value: entry.name,
    fromFullPath: entry.fullPath,
  };
}

async function submitDialog() {
  const cur = dialog.value;
  if (!cur || opBusy.value) return;
  const name = cur.value.trim();
  if (!name) {
    opError.value = t("assets.nameCannotBeEmpty");
    return;
  }
  if (name.includes("/") || name.includes("\\")) {
    opError.value = t("assets.nameCannotContainSlash");
    return;
  }

  const targetRel = cur.baseRel ? `${cur.baseRel}/${name}` : name;
  const targetFull = `assets/${targetRel}`;

  opBusy.value = true;
  opError.value = "";
  try {
    if (cur.kind === "newdir") {
      await api.createAssetDir(targetFull);
      dialog.value = null;
      expandedFolders.add(cur.baseRel);
      await refreshAssets();
      navigateTo(targetRel);
    } else {
      if (!(await ensureSaved())) return;
      const result = await api.renameAsset(cur.fromFullPath ?? targetFull, targetFull);
      dialog.value = null;
      await refreshAssets();
      emit("changed", result.affectedDocuments);
    }
  } catch (e) {
    opError.value = `${e}`;
  } finally {
    opBusy.value = false;
  }
}

async function askDelete(entry: { fullPath: string; name: string }) {
  if (opBusy.value) return;
  opBusy.value = true;
  opError.value = "";
  try {
    const refs = await api.assetReferences(entry.fullPath);
    deleteTarget.value = { path: entry.fullPath, name: entry.name, refs };
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
    selectEntry(null);
    await refreshAssets();
    emit("changed", result.affectedDocuments);
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
      selectEntry(null);
    }
    await refreshAssets();
    emit("changed", result.affectedDocuments);
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

// ---------------------------------------------------------------------------
// 生命周期
// ---------------------------------------------------------------------------

onMounted(() => {
  window.addEventListener("pointerdown", closeContextMenu);
  window.addEventListener("scroll", closeContextMenu, true);
  window.addEventListener("blur", closeContextMenu);
  window.addEventListener("resize", closeContextMenu);
  void refreshAssets();
});

onUnmounted(() => {
  window.removeEventListener("pointerdown", closeContextMenu);
  window.removeEventListener("scroll", closeContextMenu, true);
  window.removeEventListener("blur", closeContextMenu);
  window.removeEventListener("resize", closeContextMenu);
});
</script>

<template>
  <div ref="browserRoot" class="asset-browser">
    <!-- 顶部工具栏 -->
    <div class="project-toolbar">
      <!-- 导航前进后退按钮 -->
      <div class="nav-actions">
        <button
          class="ghost icon nav-btn"
          :disabled="navHistoryIndex <= 0"
          :title="t('assets.back')"
          @click="goBack"
        >
          <ChevronLeft :size="14" />
        </button>
        <button
          class="ghost icon nav-btn"
          :disabled="navHistoryIndex >= navHistory.length - 1"
          :title="t('assets.forward')"
          @click="goForward"
        >
          <ChevronRight :size="14" />
        </button>
      </div>

      <!-- 面包屑路径条 -->
      <div class="breadcrumbs-container">
        <template v-for="(crumb, idx) in breadcrumbs" :key="crumb.relPath">
          <button
            class="crumb-btn"
            :class="{ active: idx === breadcrumbs.length - 1 }"
            @click="navigateTo(crumb.relPath)"
          >
            {{ crumb.name }}
          </button>
          <span v-if="idx < breadcrumbs.length - 1" class="crumb-sep">›</span>
        </template>
      </div>

      <div class="toolbar-spacer" />

      <!-- 搜索输入框 -->
      <div class="search-box">
        <Search :size="12" class="search-icon" />
        <input
          v-model="search"
          type="text"
          :placeholder="t('assets.searchPlaceholder')"
        />
        <button
          v-if="search"
          class="ghost icon clear-btn"
          @click="search = ''"
        >
          <X :size="12" />
        </button>
      </div>

      <!-- 资产类型下拉 -->
      <EditorDropdown
        v-model="kind"
        :options="kindDropdownOptions"
        :title="t('assets.allTypes')"
      />

      <!-- 新建文件夹 -->
      <button
        class="ghost icon create-btn"
        :disabled="opBusy"
        :title="t('assets.newFolder')"
        @click="openNewDir(activeFolder)"
      >
        <Plus :size="14" />
      </button>

      <!-- 刷新 -->
      <button
        class="ghost icon refresh-btn"
        :disabled="opBusy"
        :title="t('assets.refresh')"
        @click="refreshAssets"
      >
        <RefreshCw :size="13" />
      </button>
    </div>

    <!-- 操作错误提示 -->
    <div v-if="opError" class="asset-op-error" :title="opError">{{ opError }}</div>

    <!-- 自定义 Editor UI 对话框：新建文件夹 / 重命名 -->
    <div v-if="dialog" class="asset-dialog-card">
      <div class="dialog-title">{{ dialog.title }}</div>
      <input
        v-model="dialog.value"
        type="text"
        class="dialog-input"
        autofocus
        @keydown.enter.prevent="submitDialog"
        @keydown.esc.prevent="dialog = null"
      />
      <div class="dialog-actions">
        <button class="ghost" @click="dialog = null">{{ t("common.cancel", { default: "取消" }) }}</button>
        <button class="primary" :disabled="opBusy || !dialog.value.trim()" @click="submitDialog">
          {{ t("common.confirm", { default: "确定" }) }}
        </button>
      </div>
    </div>

    <!-- 自定义 Editor UI 对话框：删除确认 -->
    <div v-if="deleteTarget" class="asset-dialog-card">
      <div class="dialog-title">{{ t("assets.confirmDelete", { path: deleteTarget.path }) }}</div>
      <div v-if="deleteTarget.refs.length" class="delete-warning">
        <div class="warning-header">{{ t("assets.warningRef", { count: deleteTarget.refs.length }) }}</div>
        <div v-for="ref in deleteTarget.refs.slice(0, 5)" :key="ref" class="warning-ref">
          {{ ref }}
        </div>
        <div v-if="deleteTarget.refs.length > 5" class="warning-ref">
          {{ t("assets.moreRefs", { count: deleteTarget.refs.length - 5 }) }}
        </div>
      </div>
      <div class="dialog-actions">
        <button class="ghost" @click="deleteTarget = null">{{ t("common.cancel", { default: "取消" }) }}</button>
        <button class="danger" :disabled="opBusy" @click="confirmDelete">
          {{ t("assets.delete") }}
        </button>
      </div>
    </div>

    <!-- 双栏主体视图 -->
    <div class="project-body">
      <!-- 左栏：文件夹层级树 -->
      <div class="folder-tree-pane" :style="{ width: `${treeWidth}px` }">
        <div
          v-for="row in visibleFolderRows"
          :key="row.relPath || 'root'"
          class="folder-tree-row"
          :class="{
            active: activeFolder === row.relPath,
            'drop-target': dragOverFolder === row.relPath,
          }"
          :style="{ paddingLeft: `${4 + row.depth * 14}px` }"
          :draggable="!!row.relPath"
          @click="onFolderRowClick(row.relPath)"
          @dblclick="toggleFolderExpanded(row.relPath)"
          @dragstart="onFolderDragStart(row, $event)"
          @dragover="onFolderDragOver(row.relPath, $event)"
          @dragleave="onFolderDragLeave"
          @drop.stop.prevent="onFolderDrop(row.relPath, $event)"
          @contextmenu.prevent.stop="openFolderContextMenu(row, $event)"
        >
          <span
            class="folder-foldout"
            :class="{ hidden: !row.hasChildren }"
            @click.stop="toggleFolderExpanded(row.relPath)"
          >
            <ChevronDown v-if="isFolderExpanded(row.relPath)" :size="12" />
            <ChevronRight v-else :size="12" />
          </span>
          <component
            :is="isFolderExpanded(row.relPath) || activeFolder === row.relPath ? FolderOpen : Folder"
            :size="14"
            class="folder-icon"
          />
          <span class="folder-name">{{ row.name }}</span>
        </div>
      </div>

      <!-- 垂直 Splitter -->
      <div
        class="project-splitter"
        title="拖动调整文件夹树宽度"
        @pointerdown="startSplitterResize"
      />

      <!-- 右栏：内容区 (ListArea) -->
      <div
        class="folder-contents-pane"
        :class="{ 'drop-target': dragOverFolder === activeFolder }"
        @click="onEmptyAreaClick"
        @contextmenu.prevent.stop="openEmptyAreaContextMenu"
        @dragover="onContentsDragOver"
        @dragleave="onContentsDragLeave"
        @drop.stop.prevent="onContentsDrop"
      >
        <!-- 列表视图（滑块处于 16px） -->
        <div v-if="iconSize <= 18" class="contents-list-view">
          <div
            v-for="entry in currentEntries"
            :key="entry.id"
            class="content-list-row"
            :class="{
              active: selectedEntry?.id === entry.id,
              'drop-target': dragOverFolder === entry.relPath && entry.isDir,
            }"
            :draggable="true"
            @click.stop="onEntryClick(entry)"
            @dblclick.stop="onEntryDoubleClick(entry)"
            @dragstart="onEntryDragStart(entry, $event)"
            @dragover="entry.isDir ? onFolderDragOver(entry.relPath, $event) : undefined"
            @dragleave="entry.isDir ? onFolderDragLeave() : undefined"
            @drop.stop.prevent="entry.isDir ? onFolderDrop(entry.relPath, $event) : undefined"
            @contextmenu.prevent.stop="openEntryContextMenu(entry, $event)"
          >
            <component
              :is="getEntryIcon(entry)"
              :size="14"
              class="entry-icon"
              :class="getEntryIconClass(entry)"
            />
            <span class="entry-name">{{ entry.name }}</span>
            <span v-if="entry.isDir" class="entry-badge folder-badge">{{ t("assets.folder") }}</span>
            <span v-else-if="entry.kind" class="entry-badge">{{ entry.kind }}</span>
            <span class="entry-size">{{ entry.sizeBytes != null ? formatBytes(entry.sizeBytes) : "" }}</span>
          </div>
        </div>

        <!-- 网格卡片视图（滑块 > 18px） -->
        <div
          v-else
          class="contents-grid-view"
          :style="{ '--icon-size': `${iconSize}px` }"
        >
          <div
            v-for="entry in currentEntries"
            :key="entry.id"
            class="content-grid-card"
            :class="{
              active: selectedEntry?.id === entry.id,
              'drop-target': dragOverFolder === entry.relPath && entry.isDir,
            }"
            :draggable="true"
            :title="entry.name"
            @click.stop="onEntryClick(entry)"
            @dblclick.stop="onEntryDoubleClick(entry)"
            @dragstart="onEntryDragStart(entry, $event)"
            @dragover="entry.isDir ? onFolderDragOver(entry.relPath, $event) : undefined"
            @dragleave="entry.isDir ? onFolderDragLeave() : undefined"
            @drop.stop.prevent="entry.isDir ? onFolderDrop(entry.relPath, $event) : undefined"
            @contextmenu.prevent.stop="openEntryContextMenu(entry, $event)"
          >
            <div class="card-icon-wrapper">
              <component
                :is="getEntryIcon(entry)"
                :size="Math.min(iconSize, 56)"
                class="card-icon"
                :class="getEntryIconClass(entry)"
              />
            </div>
            <span class="card-label">{{ entry.name }}</span>
          </div>
        </div>

        <!-- 空目录提示 -->
        <div v-if="currentEntries.length === 0" class="empty-hint">
          {{ isSearching ? t("assets.noMatch") : t("assets.emptyFolder") }}
        </div>
      </div>
    </div>

    <!-- 底部状态栏 -->
    <div class="project-bottom-bar">
      <div class="status-summary" :title="statusLabel">
        {{ statusLabel }}
      </div>
      <div class="slider-wrapper" :title="t('assets.iconSize')">
        <input
          v-model.number="iconSize"
          type="range"
          min="16"
          max="96"
          step="4"
          class="icon-size-slider"
        />
      </div>
    </div>
  </div>

  <!-- 右键上下文菜单 -->
  <Teleport to="body">
    <div
      v-if="contextMenu"
      class="entity-context-menu asset-context-menu"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @pointerdown.stop
      @click.stop
      @contextmenu.prevent
    >
      <!-- 双击打开文档 -->
      <button
        v-if="contextMenu.isItem && !contextMenu.isDir && isDocumentPath(contextMenu.fullPath)"
        @click="emit('open', contextMenu.fullPath); closeContextMenu()"
      >
        <span>{{ t("assets.openDoc") }}</span>
      </button>

      <!-- 新建文件夹 -->
      <button
        v-if="contextMenu.isDir"
        @click="openNewDir(contextMenu.relPath); closeContextMenu()"
      >
        <span>{{ t("assets.newFolder") }}</span>
      </button>

      <!-- 重命名 -->
      <button
        v-if="contextMenu.isItem && contextMenu.relPath"
        :disabled="opBusy"
        @click="openRename(contextMenu); closeContextMenu()"
      >
        <span>{{ t("assets.rename") }}</span>
      </button>

      <!-- 复制资产 (Ctrl+D) -->
      <button
        v-if="contextMenu.isItem && !contextMenu.isDir"
        :disabled="opBusy"
        @click="duplicateAsset(contextMenu.relPath); closeContextMenu()"
      >
        <span>{{ t("assets.duplicate") }}</span>
      </button>

      <!-- 复制路径 -->
      <button @click="copyPath(contextMenu.fullPath); closeContextMenu()">
        <span>{{ t("assets.copyPath") }}</span>
      </button>

      <!-- 复制 GUID -->
      <button
        v-if="contextMenu.isItem && !contextMenu.isDir"
        @click="copyAssetGuid(contextMenu.fullPath); closeContextMenu()"
      >
        <span>{{ t("assets.copyGuid") }}</span>
      </button>

      <!-- 刷新 -->
      <button
        v-if="!contextMenu.isItem"
        @click="refreshAssets(); closeContextMenu()"
      >
        <span>{{ t("assets.refresh") }}</span>
      </button>

      <!-- 删除 -->
      <button
        v-if="contextMenu.isItem && contextMenu.relPath"
        class="danger"
        :disabled="opBusy"
        @click="askDelete(contextMenu); closeContextMenu()"
      >
        <span>{{ t("assets.delete") }}</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.asset-browser {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  background: var(--bg-sunken);
  color: var(--text);
  font-size: 12px;
  user-select: none;
}

/* 顶部工具栏 */
.project-toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 6px;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.nav-actions {
  display: flex;
  align-items: center;
  gap: 1px;
}

.nav-btn {
  padding: 2px 4px;
  height: 20px;
  color: var(--text-soft);
}
.nav-btn:disabled {
  opacity: 0.35;
  cursor: default;
}

.breadcrumbs-container {
  display: flex;
  align-items: center;
  gap: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  margin-left: 2px;
}

.crumb-btn {
  background: transparent;
  border: none;
  padding: 1px 4px;
  color: var(--muted);
  cursor: pointer;
  border-radius: 2px;
  font-size: 11px;
}
.crumb-btn:hover {
  color: var(--text);
  background: var(--hover-bg);
}
.crumb-btn.active {
  color: var(--text);
  font-weight: 500;
}

.crumb-sep {
  color: var(--muted);
  font-size: 11px;
  opacity: 0.7;
}

.toolbar-spacer {
  flex: 1 1 auto;
}

.search-box {
  position: relative;
  display: flex;
  align-items: center;
  width: 140px;
  flex-shrink: 0;
}

.search-icon {
  position: absolute;
  left: 6px;
  color: var(--muted);
  pointer-events: none;
}

.search-box input {
  width: 100%;
  height: 20px;
  padding: 0 20px 0 22px;
  font-size: 11px;
  background: var(--bg-sunken);
  border: 1px solid var(--border);
  border-radius: 3px;
  color: var(--text);
}

.clear-btn {
  position: absolute;
  right: 2px;
  padding: 2px;
  height: 16px;
  color: var(--muted);
}

.create-btn,
.refresh-btn {
  padding: 2px 5px;
  height: 20px;
  color: var(--text-soft);
}

/* 错误条目 */
.asset-op-error {
  background: rgba(239, 68, 68, 0.15);
  border-bottom: 1px solid #ef4444;
  color: #fca5a5;
  padding: 4px 8px;
  font-size: 11px;
  flex-shrink: 0;
}

/* 对话框卡片 */
.asset-dialog-card {
  padding: 8px 12px;
  background: var(--panel-alt);
  border-bottom: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex-shrink: 0;
}

.dialog-title {
  font-size: 12px;
  font-weight: 500;
  color: var(--text);
}

.dialog-input {
  height: 22px;
  padding: 0 6px;
  background: var(--bg-sunken);
  border: 1px solid var(--border);
  border-radius: 3px;
  color: var(--text);
  font-size: 12px;
}

.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
}

.delete-warning {
  font-size: 11px;
  color: #fbbf24;
  background: rgba(245, 158, 11, 0.1);
  padding: 6px 8px;
  border-radius: 3px;
}
.warning-ref {
  color: var(--text-soft);
  font-family: ui-monospace, monospace;
  font-size: 10px;
  margin-top: 2px;
}

/* 主体视图 */
.project-body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden;
}

/* 左侧文件夹树 */
.folder-tree-pane {
  flex-shrink: 0;
  min-width: 100px;
  max-width: 360px;
  overflow-y: auto;
  overflow-x: hidden;
  background: var(--panel);
  border-right: 1px solid var(--border);
}

.folder-tree-row {
  display: flex;
  align-items: center;
  height: 20px;
  padding-right: 6px;
  gap: 4px;
  cursor: pointer;
  color: var(--text-soft);
  font-size: 11.5px;
}

.folder-tree-row:hover {
  background: var(--hover-bg);
  color: var(--text);
}

.folder-tree-row.active {
  background: var(--active-bg, #2c5d87);
  color: #fff;
}

.folder-tree-row.drop-target {
  outline: 2px dashed var(--accent, #388bfd);
  outline-offset: -2px;
}

.folder-foldout {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  color: var(--muted);
}
.folder-foldout.hidden {
  visibility: hidden;
}

.folder-icon {
  flex-shrink: 0;
  color: #dcb67a;
}

.folder-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Splitter */
.project-splitter {
  width: 4px;
  cursor: col-resize;
  background: transparent;
  flex-shrink: 0;
  transition: background 0.15s;
}
.project-splitter:hover,
.project-splitter:active {
  background: var(--accent, #388bfd);
}

/* 右侧内容区 */
.folder-contents-pane {
  flex: 1 1 auto;
  min-width: 0;
  overflow-y: auto;
  overflow-x: hidden;
  background: var(--bg-sunken);
  position: relative;
}

.folder-contents-pane.drop-target {
  background: rgba(56, 139, 253, 0.08);
  outline: 2px dashed var(--accent, #388bfd);
  outline-offset: -2px;
}

/* 列表视图 */
.contents-list-view {
  display: flex;
  flex-direction: column;
}

.content-list-row {
  display: flex;
  align-items: center;
  height: 22px;
  padding: 0 8px;
  gap: 6px;
  cursor: pointer;
  color: var(--text-soft);
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
}

.content-list-row:hover {
  background: var(--hover-bg);
  color: var(--text);
}

.content-list-row.active {
  background: var(--active-bg, #2c5d87);
  color: #fff;
}

.content-list-row.drop-target {
  outline: 2px dashed var(--accent, #388bfd);
  outline-offset: -2px;
}

.entry-icon {
  flex-shrink: 0;
}
.icon-folder {
  color: #dcb67a;
}
.icon-scene {
  color: #38bdf8;
}
.icon-prefab {
  color: #60a5fa;
}
.icon-model {
  color: #a78bfa;
}
.icon-texture {
  color: #34d399;
}
.icon-data {
  color: #cbd5e1;
}
.icon-generic {
  color: var(--muted);
}

.entry-name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11.5px;
}

.entry-badge {
  font-size: 9.5px;
  padding: 0 4px;
  border-radius: 2px;
  background: var(--panel-alt);
  color: var(--muted);
  border: 1px solid var(--border);
  text-transform: uppercase;
}

.folder-badge {
  color: #dcb67a;
  border-color: rgba(220, 182, 122, 0.3);
}

.entry-size {
  font-size: 10.5px;
  color: var(--muted);
  flex-shrink: 0;
  width: 58px;
  text-align: right;
}

/* 网格卡片视图 */
.contents-grid-view {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(calc(var(--icon-size) + 32px), 1fr));
  gap: 6px;
  padding: 8px;
  align-content: flex-start;
}

.content-grid-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 6px 4px 4px;
  border-radius: 4px;
  cursor: pointer;
  background: transparent;
  border: 1px solid transparent;
  transition: background 0.1s;
}

.content-grid-card:hover {
  background: var(--hover-bg);
}

.content-grid-card.active {
  background: var(--active-bg, #2c5d87);
  border-color: var(--accent, #388bfd);
}

.content-grid-card.drop-target {
  outline: 2px dashed var(--accent, #388bfd);
  outline-offset: -2px;
}

.card-icon-wrapper {
  display: flex;
  align-items: center;
  justify-content: center;
  width: var(--icon-size);
  height: var(--icon-size);
  margin-bottom: 4px;
}

.card-icon {
  max-width: 100%;
  max-height: 100%;
}

.card-label {
  font-size: 11px;
  color: var(--text-soft);
  text-align: center;
  width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  line-height: 1.25;
  word-break: break-all;
}
.content-grid-card.active .card-label {
  color: #fff;
}

/* 空提示 */
.empty-hint {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 80px;
  color: var(--muted);
  font-size: 12px;
}

/* 底部状态栏 */
.project-bottom-bar {
  display: flex;
  align-items: center;
  height: 22px;
  padding: 0 8px;
  background: var(--panel);
  border-top: 1px solid var(--border);
  flex-shrink: 0;
  gap: 8px;
}

.status-summary {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  color: var(--muted);
}

.slider-wrapper {
  display: flex;
  align-items: center;
  width: 68px;
  flex-shrink: 0;
}

.icon-size-slider {
  width: 100%;
  height: 10px;
  accent-color: var(--accent, #388bfd);
  cursor: pointer;
}
</style>
