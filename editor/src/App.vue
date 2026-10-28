<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, reactive, ref, shallowRef, watch } from "vue";
import {
  Box,
  Check,
  ChevronRight,
  Maximize2,
  MousePointer,
  Move,
  Pause,
  Play,
  RefreshCw,
  RotateCw,
  Square,
  StepForward,
} from "lucide-vue-next";
import AssetBrowser from "./components/AssetBrowser.vue";
import AssetInspector from "./components/AssetInspector.vue";
import AssetPicker from "./components/AssetPicker.vue";
import BuildPanel from "./components/BuildPanel.vue";
import ConsolePanel from "./components/ConsolePanel.vue";
import DockLayout from "./components/DockLayout.vue";
import EditorDropdown, { type DropdownOption } from "./components/EditorDropdown.vue";
import EditorModal from "./components/EditorModal.vue";
import HierarchyTree from "./components/HierarchyTree.vue";
import Inspector from "./components/Inspector.vue";
import PreferencesPanel from "./components/PreferencesPanel.vue";
import ProjectSettingsPanel from "./components/ProjectSettingsPanel.vue";
import ThemePanel from "./components/ThemePanel.vue";
import ViewportPanel from "./components/ViewportPanel.vue";
import { t } from "./i18n";
import {
  LAYOUT_PRESETS,
  collectTabs,
  defaultDock,
  leaf,
  removePanel,
  sanitizeDock,
  sanitizeFloating,
  showPanel,
  type DockNode,
  type FloatPanel,
} from "./dock";
import * as api from "./api";
import { ASSETS_KEY, ASSET_PICKER_KEY } from "./keys";
import {
  loadPreferences,
  savePreferences,
  type EditorPreferences,
} from "./preferences";
import {
  applyTheme,
  loadThemeState,
  saveThemeState,
  type ThemeState,
} from "./theme";
import { PREFAB_LINK_COMPONENT } from "./types";
import type {
  AssetInfo,
  AssetRef,
  BakeGiOptions,
  ComponentClipboard,
  DocKind,
  EditorWindowDescriptor,
  EntityJson,
  EntityPreset,
  HeightmapOptions,
  JsonValue,
  PlayableScene,
  PreviewLogLine,
  RenderGraphReport,
  SceneDoc,
  SceneEntry,
  ScenePackageStatus,
  SceneTemplate,
  Schema,
} from "./types";

const HISTORY_LIMIT = 200;
const COALESCE_MS = 700;
const VALIDATE_DEBOUNCE_MS = 250;

const schema = ref<Schema>({ types: [] });
const projectRoot = ref("");
const scenes = ref<SceneEntry[]>([]);
const prefabs = ref<SceneEntry[]>([]);

const sceneDropdownOptions = computed<DropdownOption[]>(() =>
  scenes.value.map((s) => ({ value: s.path, label: s.name })),
);

const layoutDropdownOptions = computed<DropdownOption[]>(() =>
  LAYOUT_PRESETS.map((lp) => ({ value: lp.id, label: lp.label })),
);
const sceneTemplates = ref<SceneTemplate[]>([]);
const presets = ref<EntityPreset[]>([]);
const assets = ref<AssetInfo[]>([]);

/** 运行 / 预览：manifest 场景与运行时包状态（构建 / 首选项面板仍会用）。 */
const playableScenes = ref<PlayableScene[]>([]);
const playSceneName = ref("");
const packageStatus = ref<ScenePackageStatus>({ exists: false, sizeBytes: 0 });
/**
 * Game view 是否在播放。
 *
 * 2026-10-27 起「运行」不再 spawn 游戏子进程：编辑器持有 engine 实例，
 * Play 由 Game view 的视口线程模拟（见 `components/ViewportPanel.vue`）。
 */
const playing = ref(false);
/** Game view 的 Play 是否暂停（暂停后画面照渲，时间停住；可单步）。 */
const paused = ref(false);

/** 编辑器首选项（本机 localStorage）。 */
const editorPrefs = ref<EditorPreferences>(loadPreferences());

/** 编辑器主题（本机 localStorage；只覆盖 CSS 变量）。 */
const themeState = ref<ThemeState>(loadThemeState());

/** 渲染图可视化（数据来自游戏进程写出的 JSON 报告）。 */
const graphReport = ref<RenderGraphReport | null>(null);
const graphError = ref("");
let graphTimer: number | undefined;

/** 顶部菜单栏。 */
const openMenu = ref<string | null>(null);

/** 离线工具：GI 烘焙 / 高度图。 */
const toolTab = ref<"gi" | "heightmap">("gi");
const toolRunning = ref(false);
const toolLog = ref("");
const giForm = reactive<BakeGiOptions>({
  rays: 64,
  bounces: 3,
  output: "assets/gi/probe_volume.bin",
  pak: "assets/scenes.pak",
  rscn: "target/scene_pack/scenes/default.rscn",
});
const heightForm = reactive<HeightmapOptions>({
  output: "assets/heightmaps/terrain.raw",
  width: 512,
  height: 512,
  seed: 24301,
  iterations: 100,
  particles: 200000,
  seaLevel: 0.7,
});

const GRAPH_NODE_W = 158;
const GRAPH_NODE_H = 64;
const GRAPH_GAP = 86;
const GRAPH_TOP = 52;
const GRAPH_HEIGHT = 260;
const PASS_COLORS: Record<string, string> = {
  shadow: "#c9a227",
  forward: "#4f9dde",
  gtao: "#5aa469",
  post: "#b06ab3",
  pt: "#de7b4f",
  rt: "#7f8fa6",
  unknown: "#8b93a7",
};
const RESOURCE_COLORS: Record<string, string> = {
  color: "#e06c75",
  depth: "#61afef",
  storage_image: "#98c379",
  storage_buffer: "#c678dd",
  other: "#9aa4b2",
};

provide(ASSETS_KEY, assets);

/** 新建时的文档种类（工具栏）。 */

const presetId = ref("empty");

/** 当前打开文档的种类。 */
const documentKind = ref<DocKind>("scene");
const currentPath = ref<string | null>(null);
const doc = ref<SceneDoc | null>(null);
/** 上次保存/加载时的序列化快照，用于精确判断「是否有未保存改动」。 */
const baseline = ref<string>("");

/** 选中的实体（最后一个为「主选中」，检查器显示它）。 */
const selection = ref<number[]>([]);
/** 选中的资产（对齐 Unity：点击 Project 资产时在 Inspector 展示其导入配置与预览）。 */
const selectedAsset = ref<AssetInfo | null>(null);

function onAssetSelect(asset: AssetInfo | null) {
  selectedAsset.value = asset;
  if (asset) {
    selection.value = [];
  }
}
/**
 * 组件剪贴板。用 `shallowRef`：整体替换、不需要深响应，
 * 也避免 Vue 对递归的 `JsonValue` 类型做深度 `UnwrapRef` 推断。
 */
const clipboard = shallowRef<ComponentClipboard | null>(null);

const status = ref("");
const error = ref("");
const validationError = ref("");
const logText = ref("");
const gameLogEntries = ref<PreviewLogLine[]>([]);
const busy = ref(false);

const documentLabel = computed(() => {
  if (currentPath.value) return currentPath.value.split("/").pop() || currentPath.value;
  return hasDoc.value ? "未保存的文档" : "无文档";
});

// ── 顶部工具栏状态（对齐 Unity） ───────────────────────────────────────────
const activeTool = ref<"select" | "move" | "rotate" | "scale">("move");
const activePivot = ref<"pivot" | "center">("pivot");
const activeSpace = ref<"global" | "local">("global");
const activeLayout = ref<string>("default");

function applyLayout(id: string) {
  activeLayout.value = id;
  const preset = LAYOUT_PRESETS.find((p) => p.id === id);
  if (preset) {
    dockLayout.value = preset.factory();
    floatingPanels.value = [];
  }
}

function onToolbarSceneSelect(path: string) {
  const scene = scenes.value.find((s) => s.path === path);
  if (scene) openDocument(scene, "scene");
}

// ---------------------------------------------------------------------------
// Docking 布局（split tree + 标签页拖拽，持久化到 localStorage）
// ---------------------------------------------------------------------------

const PANEL_IDS = [
  "sceneView",
  "gameView",
  "docs",
  "hierarchy",
  "assets",
  "inspector",
  "log",
  "renderGraph",
  "projectSettings",
  "preferences",
  "theme",
  "build",
  "tools",
  "shortcuts",
  "about",
] as const;
type PanelId = (typeof PANEL_IDS)[number];

/**
 * 面板注册表的内置回退 = Unity `EditorWindow` / `[MenuItem]` 的前端镜像。
 *
 * 正常启动时由后端 `editor_windows`（`prism_editor_api::editor_window`）覆盖；
 * 浏览器预览 / 旧后端拿不到命令时用这份，保证 UI 仍可用。
 */
const FALLBACK_WINDOWS: EditorWindowDescriptor[] = [
  { id: "sceneView", title: "场景视图", menuPath: "窗口/场景视图", shortcut: null, command: "window/toggle/sceneView" },
  { id: "gameView", title: "游戏视图", menuPath: "窗口/游戏视图", shortcut: null, command: "window/toggle/gameView" },
  { id: "docs", title: "文档", menuPath: "窗口/文档", shortcut: "Ctrl+O", command: "window/toggle/docs" },
  { id: "hierarchy", title: "层级", menuPath: "窗口/层级", shortcut: null, command: "window/toggle/hierarchy" },
  { id: "assets", title: "资产", menuPath: "窗口/资产", shortcut: null, command: "window/toggle/assets" },
  { id: "inspector", title: "检查器", menuPath: "窗口/检查器", shortcut: null, command: "window/toggle/inspector" },
  { id: "log", title: "日志", menuPath: "窗口/日志", shortcut: null, command: "window/toggle/log" },
  { id: "renderGraph", title: "渲染图", menuPath: "窗口/分析/实时渲染图", shortcut: null, command: "window/toggle/renderGraph" },
  { id: "projectSettings", title: "项目设置", menuPath: "窗口/项目设置", shortcut: null, command: "window/toggle/projectSettings" },
  { id: "preferences", title: "首选项", menuPath: "窗口/首选项", shortcut: null, command: "window/toggle/preferences" },
  { id: "theme", title: "主题", menuPath: "窗口/主题", shortcut: null, command: "window/toggle/theme" },
  { id: "build", title: "构建项目", menuPath: "窗口/构建项目", shortcut: null, command: "window/toggle/build" },
  { id: "tools", title: "离线工具", menuPath: "窗口/离线工具", shortcut: null, command: "window/toggle/tools" },
  { id: "shortcuts", title: "快捷键", menuPath: "帮助/键盘快捷键", shortcut: null, command: "window/toggle/shortcuts" },
  { id: "about", title: "关于", menuPath: "帮助/关于 PrismaRev Editor", shortcut: null, command: "window/toggle/about" },
];

const windowRegistry = ref<EditorWindowDescriptor[]>(FALLBACK_WINDOWS);

/** 标签页列表（`PANELS`）由注册表派生，顺序即「窗口」菜单顺序。 */
const PANELS = computed<Array<{ id: PanelId; title: string }>>(() =>
  windowRegistry.value
    .filter((window) => (PANEL_IDS as readonly string[]).includes(window.id))
    .map((window) => ({ id: window.id as PanelId, title: window.title })),
);
/**
 * 拉取后端面板注册表 = Unity `EditorWindow` / `[MenuItem]` 声明。
 *
 * 以 `FALLBACK_WINDOWS` 为骨架按下标覆盖：后端只改标题/菜单路径/快捷键/命令，
 * 不会因为少发一个窗口就让标签页消失。浏览器预览 / 旧后端失败时保持回退。
 */
async function loadWindowRegistry() {
  try {
    const windows = await api.fetchEditorWindows();
    const byId = new Map(windows.map((window) => [window.id, window]));
    windowRegistry.value = FALLBACK_WINDOWS.map((window) => byId.get(window.id) ?? window);
  } catch {
    // 非 Tauri 环境（纯浏览器预览）或旧后端：继续用 FALLBACK_WINDOWS。
  }
}

const DOCK_STORAGE_KEY = "prisma-editor.dock.v5";
const dockLayout = ref<DockNode | null>(defaultDock());
const floatingPanels = ref<FloatPanel[]>([]);
const visiblePanels = computed(
  () =>
    new Set([
      ...(dockLayout.value ? collectTabs(dockLayout.value) : []),
      ...floatingPanels.value.map((panel) => panel.tab),
    ]),
);

function isPanelVisible(id: PanelId): boolean {
  return visiblePanels.value.has(id);
}

function focusFloating(id: PanelId) {
  const panel = floatingPanels.value.find((item) => item.tab === id);
  if (!panel) return;
  const z = floatingPanels.value.reduce((max, item) => Math.max(max, item.z), 0) + 1;
  if (panel.z === z) return;
  floatingPanels.value = floatingPanels.value.map((item) =>
    item.tab === id ? { ...item, z } : item,
  );
}

function showPanelTab(id: PanelId) {
  if (floatingPanels.value.some((panel) => panel.tab === id)) {
    focusFloating(id);
    return;
  }
  dockLayout.value = dockLayout.value ? showPanel(dockLayout.value, id) : leaf([id]);
}

function closePanelTab(id: PanelId) {
  if (dockLayout.value) dockLayout.value = removePanel(dockLayout.value, id);
  floatingPanels.value = floatingPanels.value.filter((panel) => panel.tab !== id);
}

function togglePanel(id: PanelId) {
  if (isPanelVisible(id)) closePanelTab(id);
  else showPanelTab(id);
}

// —— 面板命令 = Unity `[MenuItem]` 的 action ——
// 后端只声明「命令 id」（`window/toggle/<panel>`），动作必须留在前端：它需要
// docking 状态、localStorage 与轮询。未知命令直接忽略（向前兼容）。
const PANEL_COMMANDS: Record<string, () => void> = Object.fromEntries(
  PANEL_IDS.map((id) => [`window/toggle/${id}`, () => togglePanel(id)]),
);

function runPanelCommand(command: string) {
  PANEL_COMMANDS[command]?.();
}

/** 把 `窗口/分析/实时渲染图` 折进菜单树，复用已有的同名子菜单。 */
function addMenuPath(items: MenuItem[], path: string[], item: MenuItem) {
  if (!path.length) {
    items.push(item);
    return;
  }
  const [head, ...rest] = path;
  let group = items.find((entry) => entry.label === head && entry.children);
  if (!group) {
    group = { label: head, children: [] };
    items.push(group);
  }
  addMenuPath(group.children ?? [], rest, item);
}

/**
 * 「窗口」与「帮助」菜单由注册表的 `menuPath` 派生
 * （= Unity `[MenuItem("窗口/文档")]`），顺序即注册表顺序。
 */
const panelMenuRoots = computed(() => {
  const window: MenuItem[] = [];
  const help: MenuItem[] = [];
  for (const descriptor of windowRegistry.value) {
    const parts = descriptor.menuPath
      .split("/")
      .map((part) => part.trim())
      .filter(Boolean);
    const root = parts[0] === "帮助" ? help : window;
    addMenuPath(root, parts.slice(1, -1), {
      label: parts[parts.length - 1] ?? descriptor.title,
      shortcut: descriptor.shortcut ?? undefined,
      checked: () => isPanelVisible(descriptor.id as PanelId),
      action: () => runPanelCommand(descriptor.command),
    });
  }
  return { window, help };
});

// 面板被关闭/重置后停止轮询；重新显示时按需加载数据。
watch(visiblePanels, (next, previous) => {
  const graphWasVisible = previous?.has("renderGraph") ?? false;
  const graphVisible = next.has("renderGraph");
  if (graphVisible && !graphWasVisible) {
    void refreshGraph();
    startGraphPolling();
  } else if (!graphVisible && graphWasVisible) {
    stopGraphPolling();
  }

});

function loadLayout() {
  try {
    const known = PANELS.value.map((panel) => panel.id);
    const raw = localStorage.getItem(DOCK_STORAGE_KEY);
    if (raw) {
      const saved = JSON.parse(raw) as { dock?: unknown; floating?: unknown };
      const dock = sanitizeDock(saved.dock, known);
      const docked = new Set(dock ? collectTabs(dock) : []);
      // 旧布局里没有视口面板（v4 及更早）：把两个视口补进默认中间列，
      // 而不是让用户升级后看不到画面。
      dockLayout.value =
        dock && docked.has("sceneView")
          ? dock
          : showPanel(dock ?? defaultDock(), "sceneView");
      floatingPanels.value = sanitizeFloating(saved.floating, known).filter(
        (panel) => !docked.has(panel.tab),
      );
      return;
    }
  } catch {
    // 损坏的布局直接回退到默认值。
  }
}

function saveLayout() {
  try {
    localStorage.setItem(
      DOCK_STORAGE_KEY,
      JSON.stringify({ version: 4, dock: dockLayout.value, floating: floatingPanels.value }),
    );
  } catch {
    // localStorage 不可用时只在当前会话生效。
  }
}

let saveLayoutTimer: number | undefined;
watch(
  [dockLayout, floatingPanels],
  () => {
    window.clearTimeout(saveLayoutTimer);
    saveLayoutTimer = window.setTimeout(saveLayout, 200);
  },
  { deep: true },
);

function resetLayout() {
  dockLayout.value = defaultDock();
  floatingPanels.value = [];
  try {
    localStorage.removeItem(DOCK_STORAGE_KEY);
    localStorage.removeItem("prisma-editor.dock.v4");
  } catch {
    // ignore
  }
}

watch(logText, (text) => {
  if (!text) return;
  if (!isPanelVisible("log")) showPanelTab("log");
  scrollLogsToEnd();
});

const past = ref<SceneDoc[]>([]);
const future = ref<SceneDoc[]>([]);
let lastKey = "";
let lastTime = 0;

const primary = computed(() =>
  selection.value.length ? selection.value[selection.value.length - 1] : null,
);

const dirty = computed(
  () => doc.value !== null && JSON.stringify(doc.value) !== baseline.value,
);

const canUndo = computed(() => past.value.length > 0);
const canRedo = computed(() => future.value.length > 0);
const hasDoc = computed(() => doc.value !== null);
const hasPath = computed(() => currentPath.value !== null);
const canSave = computed(() => hasDoc.value && validationError.value === "");
const isScene = computed(() => documentKind.value === "scene");

const selectedEntity = computed<EntityJson | null>(() => {
  if (!doc.value || primary.value === null) return null;
  return doc.value.entities[primary.value] ?? null;
});

/** 「跳到使用处」时高亮的组件 / 字段。 */
const highlightComponent = ref<string | null>(null);
const highlightField = ref<string | null>(null);

function prefabLinkOf(entity: EntityJson | null): string | null {
  const link = entity?.components?.[PREFAB_LINK_COMPONENT] as { prefab?: string } | undefined;
  return typeof link?.prefab === "string" ? link.prefab : null;
}

/** 主选中实体所属预制体（若有）。 */
const prefabLink = computed(() => prefabLinkOf(selectedEntity.value));

/** 当前文档里每个预制体各自的实例数。 */
const instanceCounts = computed<Record<string, number>>(() => {
  const counts: Record<string, number> = {};
  for (const entity of doc.value?.entities ?? []) {
    const link = prefabLinkOf(entity);
    if (link) counts[link] = (counts[link] ?? 0) + 1;
  }
  return counts;
});

/** 文档列表项对应的预制体在当前文档里的实例数（按路径后缀匹配）。 */
function instanceCountFor(entry: SceneEntry): number {
  const normalized = entry.path.replace(/\\/g, "/");
  const key = Object.keys(instanceCounts.value).find((link) => normalized.endsWith(link));
  return key ? instanceCounts.value[key] : 0;
}

function prefabLinkFor(entry: SceneEntry): string | null {
  const normalized = entry.path.replace(/\\/g, "/");
  return Object.keys(instanceCounts.value).find((link) => normalized.endsWith(link)) ?? null;
}

function fail(e: unknown) {
  error.value = `${e}`;
}

// ---------------------------------------------------------------------------
// 资产管理器（模态选择器）
// ---------------------------------------------------------------------------

interface PickerState {
  current: string;
  resolve: (value: string | null) => void;
}

const pickerState = ref<PickerState | null>(null);

function openAssetPicker(current: string): Promise<string | null> {
  return new Promise((resolve) => {
    pickerState.value = { current, resolve };
  });
}
provide(ASSET_PICKER_KEY, openAssetPicker);

function resolvePicker(value: string | null) {
  pickerState.value?.resolve(value);
  pickerState.value = null;
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

async function runValidation() {
  if (!doc.value) {
    validationError.value = "";
    return;
  }
  try {
    await api.validateDocument(doc.value);
    validationError.value = "";
  } catch (e) {
    validationError.value = `${e}`;
  }
}

let validateTimer: number | undefined;
watch(doc, () => {
  if (validateTimer !== undefined) window.clearTimeout(validateTimer);
  validateTimer = window.setTimeout(() => void runValidation(), VALIDATE_DEBOUNCE_MS);
});

// ---------------------------------------------------------------------------
// 文档 / 历史
// ---------------------------------------------------------------------------

/** 选择状态跟随实体数量收敛（删除/撤销后下标可能失效）。 */
function clampSelection() {
  if (!doc.value) {
    selection.value = [];
    return;
  }
  const count = doc.value.entities.length;
  const kept = selection.value.filter((index) => index < count);
  selection.value = kept.length ? kept : count ? [0] : [];
}

function resetHistory(loaded: SceneDoc, path: string | null, kind: DocKind) {
  past.value = [];
  future.value = [];
  doc.value = loaded;
  currentPath.value = path;
  documentKind.value = kind;
  baseline.value = JSON.stringify(loaded);
  lastKey = "";
  lastTime = 0;
  selection.value = loaded.entities.length ? [0] : [];
  void runValidation();
}

/**
 * 提交一个新文档版本。`editKey` 相同且间隔 <700ms 的连续编辑
 * （例如拖数字框）会合并成一次撤销。
 */
function commit(next: SceneDoc, editKey: string) {
  if (!doc.value) return;
  const now = performance.now();
  const coalesce = editKey !== "" && editKey === lastKey && now - lastTime < COALESCE_MS;
  if (!coalesce) {
    past.value = [...past.value, doc.value].slice(-HISTORY_LIMIT);
  }
  future.value = [];
  doc.value = next;
  lastKey = editKey;
  lastTime = now;
}

/** 不记入历史的内部更新（反射写入回填规范化值）。 */
function silentUpdate(next: SceneDoc) {
  doc.value = next;
}

/**
 * 执行「把整份文档交给后端改写」的异步命令。
 *
 * 后端拿到的是调用瞬间的快照，返回的也是基于这份快照的整份文档。命令在
 * `await` 期间用户仍会继续拖动字段 / 就地重命名（这些编辑是本地即时提交的），
 * 直接 `commit` 返回的旧快照会把这些编辑连同撤销栈一起丢掉。
 *
 * 因此提交前比较文档内容：没被改过才提交；被改过就用最新文档重跑一次。
 * 连续几次都被打断就放弃并提示，绝不覆盖新内容。
 */
async function runDocCommand<T>(
  run: (snapshot: SceneDoc) => Promise<T>,
  apply: (result: T) => SceneDoc,
  editKey: string,
): Promise<T | null> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (!doc.value) return null;
    const snapshot = doc.value;
    const expected = JSON.stringify(snapshot);
    const result = await run(snapshot);
    if (doc.value && JSON.stringify(doc.value) === expected) {
      commit(apply(result), editKey);
      return result;
    }
  }
  status.value = "文档在操作期间又被修改，已放弃这次操作，请重试";
  return null;
}

function undo() {
  if (!past.value.length || !doc.value) return;
  const previous = past.value[past.value.length - 1];
  past.value = past.value.slice(0, -1);
  future.value = [...future.value, doc.value];
  doc.value = previous;
  lastKey = "";
  clampSelection();
}

function redo() {
  if (!future.value.length || !doc.value) return;
  const next = future.value[future.value.length - 1];
  future.value = future.value.slice(0, -1);
  past.value = [...past.value, doc.value];
  doc.value = next;
  lastKey = "";
  clampSelection();
}

interface ModalState {
  open: boolean;
  title: string;
  message: string;
  hasInput: boolean;
  defaultValue: string;
  placeholder: string;
  confirmText: string;
  cancelText: string;
  danger: boolean;
  resolve?: (value: any) => void;
}

const modalState = ref<ModalState>({
  open: false,
  title: "",
  message: "",
  hasInput: false,
  defaultValue: "",
  placeholder: "",
  confirmText: "确定",
  cancelText: "取消",
  danger: false,
});

function showConfirm(
  title: string,
  message: string,
  options?: { danger?: boolean; confirmText?: string; cancelText?: string },
): Promise<boolean> {
  return new Promise((resolve) => {
    modalState.value = {
      open: true,
      title,
      message,
      hasInput: false,
      defaultValue: "",
      placeholder: "",
      confirmText: options?.confirmText ?? t("common.confirm"),
      cancelText: options?.cancelText ?? t("common.cancel"),
      danger: options?.danger ?? false,
      resolve: (val) => resolve(Boolean(val)),
    };
  });
}

function showPrompt(
  title: string,
  message: string,
  defaultValue = "",
  placeholder = "",
): Promise<string | null> {
  return new Promise((resolve) => {
    modalState.value = {
      open: true,
      title,
      message,
      hasInput: true,
      defaultValue,
      placeholder,
      confirmText: t("common.confirm"),
      cancelText: t("common.cancel"),
      danger: false,
      resolve: (val) => resolve(typeof val === "string" ? val : null),
    };
  });
}

function onModalConfirm(val?: string) {
  modalState.value.open = false;
  modalState.value.resolve?.(modalState.value.hasInput ? (val ?? "") : true);
}

function onModalCancel() {
  modalState.value.open = false;
  modalState.value.resolve?.(modalState.value.hasInput ? null : false);
}

async function confirmDiscard(): Promise<boolean> {
  if (!dirty.value) return true;
  return await showConfirm(
    t("dialog.discardTitle"),
    t("dialog.discardMsg"),
    { danger: true },
  );
}

// ---------------------------------------------------------------------------
// 文档文件
// ---------------------------------------------------------------------------

async function refreshDocuments() {
  const [sceneList, prefabList] = await Promise.all([
    api.listDocuments("scene"),
    api.listDocuments("prefab"),
  ]);
  scenes.value = sceneList;
  prefabs.value = prefabList;
}

/** 引擎/数据层版本（`editor_engine_version`，「关于」面板用）。 */
const engineVersionText = ref("");

async function bootstrap() {
  try {
    schema.value = await api.fetchSchema();
    projectRoot.value = await api.fetchProjectRoot();
    // 版本是可选展示；失败不影响编辑。
    engineVersionText.value = await api.engineVersion().catch(() => "");
    sceneTemplates.value = await api.documentTemplates("scene");
    presets.value = await api.fetchPresets();
    if (presets.value.length) presetId.value = presets.value[0].id;
    await refreshDocuments();
    // 资产列表是可选增强；失败不影响编辑。
    assets.value = await api.listAssetsInfo().catch(() => []);
    await refreshPlayable();
    status.value = "就绪";
  } catch (e) {
    fail(e);
  }
}

async function openDocument(entry: SceneEntry, kind: DocKind) {
  if (!(await confirmDiscard())) return;
  try {
    const loaded = await api.readDocument(entry.path);
    resetHistory(loaded, entry.path, kind);
    if (kind === "scene" && playableScenes.value.some((s) => s.name === entry.name)) {
      playSceneName.value = entry.name;
    }
    error.value = "";
    status.value = `已打开 ${entry.fileName}`;
  } catch (e) {
    fail(e);
  }
}

function docKindOfPath(path: string): DocKind | null {
  const lower = path.toLowerCase();
  if (lower.endsWith(".scene.json")) return "scene";
  if (lower.endsWith(".prefab.json")) return "prefab";
  return null;
}

/** 资产管理器双击场景/预制体：直接作为文档打开。 */
async function openAssetPath(path: string) {
  const kind = docKindOfPath(path);
  if (!kind) return;
  const fileName = path.split("/").pop() ?? path;
  const suffix = kind === "scene" ? ".scene.json" : ".prefab.json";
  const name = fileName.toLowerCase().endsWith(suffix)
    ? fileName.slice(0, -suffix.length)
    : fileName;
  const root = projectRoot.value.replace(/[\\/]+$/, "");
  await openDocument({ name, fileName, path: root ? `${root}/${path}` : path }, kind);
}

async function reload() {
  if (!currentPath.value) return;
  const list = isScene.value ? scenes.value : prefabs.value;
  const entry = list.find((s) => s.path === currentPath.value);
  if (entry) await openDocument(entry, documentKind.value);
}

async function createDocumentWithTemplate(kind: DocKind, templateId: string) {
  if (!(await confirmDiscard())) return;
  try {
    const fresh = await api.newDocument(kind, templateId);
    resetHistory(fresh, null, kind);
    error.value = "";
    status.value = kind === "scene" ? "新场景（未保存）" : "新预制体（未保存）";
  } catch (e) {
    fail(e);
  }
}

/** 菜单里的「新建场景/预制体」：使用该类文档的第一个模板。 */
async function createDocumentOf(kind: DocKind) {
  try {
    const list = await api.documentTemplates(kind);
    await createDocumentWithTemplate(kind, list[0]?.id ?? "");
  } catch (e) {
    fail(e);
  }
}

/**
 * 保存用的快照。
 *
 * 写入是异步的，期间用户还在拖动/输入；基线必须和**真正写进磁盘的那一份**
 * 完全一致。若在 `await` 之后才对 `doc` 取快照当基线，写入期间产生的编辑
 * 会被当成已保存（`dirty` 变 false），关闭或切换文档时静默丢失。
 */
function snapshotDoc(): { text: string; doc: SceneDoc } {
  const text = JSON.stringify(doc.value);
  return { text, doc: JSON.parse(text) as SceneDoc };
}

async function save(): Promise<boolean> {
  if (!doc.value) return false;
  if (!currentPath.value) return saveAs();
  const path = currentPath.value;
  const snapshot = snapshotDoc();
  try {
    await api.writeDocument(path, snapshot.doc);
    // 写入期间若已切到别的文档，别把这边的基线套到另一份文档上。
    if (currentPath.value === path) baseline.value = snapshot.text;
    error.value = "";
    status.value = `已保存 ${path}`;
    await refreshDocuments();
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function saveAs(): Promise<boolean> {
  if (!doc.value) return false;
  const suffix = documentKind.value === "scene" ? ".scene.json" : ".prefab.json";
  const name = await showPrompt(
    t("dialog.saveAsTitle"),
    `${t("dialog.saveAsMsg")} (${suffix})`,
    "untitled",
  );
  if (!name || !name.trim()) return false;
  const snapshot = snapshotDoc();
  try {
    const path = await api.writeDocumentAs(documentKind.value, name.trim(), snapshot.doc);
    currentPath.value = path;
    baseline.value = snapshot.text;
    error.value = "";
    status.value = `已另存为 ${path}`;
    await refreshDocuments();
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function ensureSavedForAssetOp(): Promise<boolean> {
  if (!hasPath.value || !dirty.value) return true;
  const ok = await save();
  if (!ok) status.value = "保存失败：资产操作已取消";
  return ok;
}

function normPath(path: string): string {
  return path.replace(/\\/g, "/");
}

/**
 * 资产重命名/移动/删除后：刷新列表；若当前文档被改写或移动，
 * 从磁盘重新载入（操作前已确保保存），避免用内存里的旧路径覆盖回去。
 */
async function onAssetsChanged(affected: string[]) {
  assets.value = await api.listAssetsInfo().catch(() => assets.value);
  const previous = currentPath.value;
  if (!previous) return;

  const previousKind = documentKind.value;
  const previousNorm = normPath(previous);
  await refreshDocuments();
  const list = previousKind === "scene" ? scenes.value : prefabs.value;

  const stillExists = list.some((item) => normPath(item.path) === previousNorm);
  const currentWasRewritten = affected.some(
    (rel) => normPath(`${projectRoot.value}/${rel}`) === previousNorm,
  );
  if (stillExists && !currentWasRewritten) return;

  let entry = list.find((item) => normPath(item.path) === previousNorm);
  if (!entry) {
    for (const rel of affected) {
      const abs = normPath(`${projectRoot.value}/${rel}`);
      const found = list.find((item) => normPath(item.path) === abs);
      if (found) {
        entry = found;
        break;
      }
    }
  }
  if (!entry) {
    // 目录整体移动时文件名不变，按显示名兜底。
    const name = previousNorm.split("/").pop()!.replace(/\.(scene|prefab)\.json$/, "");
    const sameName = list.filter((item) => item.name === name);
    if (sameName.length === 1) entry = sameName[0];
  }

  if (entry) {
    await openDocument(entry, previousKind);
    return;
  }

  if (!list.some((item) => normPath(item.path) === previousNorm)) {
    doc.value = null;
    currentPath.value = null;
    baseline.value = "";
    validationError.value = "";
    past.value = [];
    future.value = [];
    selection.value = [];
    status.value = "当前文档已不存在";
  }
}

async function renameCurrent() {
  if (!currentPath.value) return;
  const list = isScene.value ? scenes.value : prefabs.value;
  const current = list.find((s) => s.path === currentPath.value);
  const name = await showPrompt(
    t("dialog.renameTitle"),
    t("dialog.renameMsg"),
    current?.name ?? "",
  );
  if (!name || !name.trim()) return;
  try {
    const path = await api.renameDocument(currentPath.value, documentKind.value, name.trim());
    currentPath.value = path;
    error.value = "";
    status.value = `已重命名为 ${path}`;
    await refreshDocuments();
  } catch (e) {
    fail(e);
  }
}

async function deleteCurrent() {
  if (!currentPath.value) return;
  const ok = await showConfirm(
    t("dialog.deleteDocTitle"),
    t("dialog.deleteDocMsg", { path: currentPath.value }),
    { danger: true },
  );
  if (!ok) return;
  try {
    await api.deleteDocument(currentPath.value);
    doc.value = null;
    currentPath.value = null;
    baseline.value = "";
    validationError.value = "";
    past.value = [];
    future.value = [];
    selection.value = [];
    error.value = "";
    status.value = "已删除";
    await refreshDocuments();
  } catch (e) {
    fail(e);
  }
}

async function cook() {
  if (!doc.value || !isScene.value) return;
  // cook 读的是磁盘上的文件，先落盘。
  if (dirty.value || !currentPath.value) {
    const ok = await save();
    if (!ok || !currentPath.value) return;
  }
  busy.value = true;
  status.value = "cooking…";
  try {
    logText.value = (await api.cookScene(currentPath.value)) || "（无输出）";
    error.value = "";
    status.value = "cook 完成";
  } catch (e) {
    logText.value = `${e}`;
    status.value = "cook 失败";
  } finally {
    busy.value = false;
  }
}

// ---------------------------------------------------------------------------
// 预制体
// ---------------------------------------------------------------------------

async function insertPrefab(entry: SceneEntry, parent: number | null = primary.value) {
  if (!doc.value) {
    fail("请先打开一个场景");
    return;
  }
  if (!isScene.value) {
    fail("只能在场景中实例化预制体");
    return;
  }
  busy.value = true;
  try {
    const result = await runDocCommand(
      (snapshot) => api.instantiatePrefab(snapshot, entry.path, parent),
      (inserted) => inserted.doc,
      `insert-prefab:${entry.path}#${Date.now()}`,
    );
    if (!result) return;
    if (result.inserted.length) selection.value = [result.inserted[0]];
    error.value = "";
    status.value = `已实例化 ${entry.name}（${result.inserted.length} 个实体）`;
  } catch (e) {
    fail(e);
  } finally {
    busy.value = false;
  }
}

async function insertPrefabAt(path: string, parent: number | null) {
  const root = projectRoot.value.replace(/[\\/]+$/, "");
  const absolute = /^[A-Za-z]:[\\/]|^\//.test(path)
    ? path
    : `${root}/${path}`;
  const entry =
    prefabs.value.find((item) => normPath(item.path) === normPath(absolute)) ??
    ({
      name: (path.split("/").pop() ?? path).replace(/\.prefab\.json$/, ""),
      fileName: path.split("/").pop() ?? path,
      path: absolute,
    } satisfies SceneEntry);
  await insertPrefab(entry, parent);
}

async function revertPrefab(index: number) {
  if (!doc.value || !prefabLink.value) return;
  const link = prefabLink.value;
  try {
    const result = await runDocCommand(
      (snapshot) => api.revertPrefabInstance(snapshot, index, link),
      (reverted) => reverted.doc,
      `revert-prefab:${link}#${Date.now()}`,
    );
    if (!result) return;
    selection.value = [result.index];
    error.value = "";
    status.value = `已从 ${link} 还原`;
  } catch (e) {
    fail(e);
  }
}

async function applyPrefab(index: number) {
  if (!doc.value || !prefabLink.value) return;
  const link = prefabLink.value;
  const ok = await showConfirm(
    t("dialog.applyPrefabTitle"),
    t("dialog.applyPrefabMsg", { link }),
    { danger: true },
  );
  if (!ok) return;
  try {
    // `PrefabUtility.ApplyPrefabInstance`：提取子树 + 写回预制体文件。
    await api.applyPrefabInstance(doc.value, index);
    error.value = "";
    status.value = `已应用到 ${link}`;
    await refreshDocuments();
  } catch (e) {
    fail(e);
  }
}

async function unlinkPrefab(index: number) {
  if (!doc.value) return;
  try {
    const next = await runDocCommand(
      (snapshot) => api.unlinkPrefabInstance(snapshot, index),
      (unlinked) => unlinked,
      `unlink-prefab:${index}#${Date.now()}`,
    );
    if (!next) return;
    error.value = "";
    status.value = "已解除预制体链接";
  } catch (e) {
    fail(e);
  }
}

/** 把这个文档里所有该预制体的实例同步一遍。 */
async function syncPrefab(entry: SceneEntry) {
  if (!doc.value) return;
  const link = prefabLinkFor(entry);
  if (!link) return;
  busy.value = true;
  try {
    const next = await runDocCommand(
      (snapshot) => api.syncPrefabInstances(snapshot, link),
      (synced) => synced,
      `sync-prefab:${link}#${Date.now()}`,
    );
    if (!next) return;
    error.value = "";
    status.value = `已同步 ${instanceCounts.value[link]} 个实例`;
  } catch (e) {
    fail(e);
  } finally {
    busy.value = false;
  }
}

/** 资产生成处的「跳到该字段」：打开对应文档、选中实体、高亮组件/字段。 */
async function jumpToUsage(ref: AssetRef) {
  if (ref.kind === "manifest") {
    // 清单条目没有实体/字段可定位，只提示它登记在构建清单里。
    status.value = `${ref.entity} 在 ${ref.scene} 登记（path = ${ref.value}）`;
    return;
  }
  if (currentPath.value !== ref.scenePath) {
    if (!(await confirmDiscard())) return;
    try {
      const loaded = await api.readDocument(ref.scenePath);
      resetHistory(
        loaded,
        ref.scenePath,
        ref.scenePath.endsWith(".prefab.json") ? "prefab" : "scene",
      );
    } catch (e) {
      fail(e);
      return;
    }
  }
  selection.value = [ref.entityIndex];
  selectedAsset.value = null;
  highlightComponent.value = ref.component;
  highlightField.value = ref.field;
  error.value = "";
  status.value = `定位 ${ref.scene} · #${ref.entityIndex} ${ref.component.split("::").pop()}.${ref.field}`;
  await nextTick();
  const target =
    document.querySelector(`[data-field="${ref.field}"]`) ??
    document.querySelector(".card.highlight");
  target?.scrollIntoView({ block: "center" });
}

async function createPrefabAt(folder: string, index: number, name: string) {
  if (!doc.value) return;
  const entity = doc.value.entities[index];
  const prefabName = (name || entity?.name || "prefab").trim();
  if (!prefabName) return;
  try {
    const path = await api.createPrefabAt(doc.value, index, folder, prefabName);
    await refreshDocuments();
    assets.value = await api.listAssetsInfo().catch(() => assets.value);
    error.value = "";
    status.value = `已保存预制体 ${path}`;
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------------------
// 层级 / 选择
// ---------------------------------------------------------------------------

/** 场景视图点选 → 与层级 / 检查器共用同一份选中状态。 */
function onViewportSelect(index: number | null) {
  onSelect(index === null ? [] : [index]);
}

/**
 * 拖动平移手柄 → 写回文档的 `transform.position`（局部坐标，后端已经换算过）。
 *
 * 与检查器一致：本地即时提交保证跟手，`editKey` 用同一条
 * `drag-move:<下标>` 让整段拖动合并成一次撤销（连续提交间隔 < 700ms）。
 * 视口画面不靠这次写回 —— 引擎侧已经直接改了 `LocalTransform`。
 */
function onViewportMove(position: [number, number, number]) {
  if (!doc.value || primary.value === null) return;
  const index = primary.value;
  const entity = doc.value.entities[index];
  if (!entity) return;
  const entities = doc.value.entities.slice();
  entities[index] = {
    ...entity,
    transform: { ...entity.transform, position: [...position] },
  };
  commit({ ...doc.value, entities }, `drag-move:${index}`);
}

function onSelect(indices: number[]) {
  selection.value = indices.length ? indices : [];
  if (indices.length) {
    selectedAsset.value = null;
  }
  // 手动选择时清掉「跳到使用处」的高亮。
  highlightComponent.value = null;
  highlightField.value = null;
}

async function onReparent(child: number, parent: number | null) {
  if (!doc.value) return;
  try {
    const next = await runDocCommand(
      (snapshot) => api.reparentEntity(snapshot, child, parent),
      (reparented) => reparented,
      `reparent:${child}->${parent ?? "root"}#${Date.now()}`,
    );
    if (!next) return;
    error.value = "";
    status.value = parent === null ? "已变为根实体" : `已挂到实体 #${parent}`;
  } catch (e) {
    fail(e);
  }
}

async function onMove(child: number, at: number, parent: number | null) {
  if (!doc.value) return;
  try {
    const result = await runDocCommand(
      (snapshot) => api.moveEntity(snapshot, child, at, parent),
      (moved) => moved.doc,
      `move:${child}@${at}#${Date.now()}`,
    );
    if (!result) return;
    selection.value = [result.index];
    error.value = "";
    status.value = "已重新排序";
  } catch (e) {
    fail(e);
  }
}

function onRenameEntity(index: number, name: string) {
  if (!doc.value) return;
  const entities = doc.value.entities.slice();
  const entity = entities[index];
  if (!entity) return;
  entities[index] = { ...entity, name };
  commit({ ...doc.value, entities }, `rename-entity:${index}#${Date.now()}`);
}

function uniqueName(base: string): string {
  if (!doc.value) return base;
  const names = new Set(doc.value.entities.map((entity) => entity.name));
  if (!names.has(base)) return base;
  let suffix = 2;
  while (names.has(`${base}_${suffix}`)) suffix += 1;
  return `${base}_${suffix}`;
}

function onEntityUpdate(entity: EntityJson, editKey: string) {
  if (!doc.value || primary.value === null) return;
  const entities = doc.value.entities.slice();
  entities[primary.value] = entity;
  commit({ ...doc.value, entities }, editKey);
}

/**
 * 反射写入成功后的规范化回填（不新增撤销步）。
 *
 * `index` / `pending` 来自发起编辑的那一刻：异步往返期间用户可能切换选中、
 * 删除或移动实体，所以先确认「这个位置上的组件仍是我们刚写下的乐观值」，
 * 否则丢弃 —— 绝不能按当前选中项回填（那会把值写进别人的实体）。
 */
function onComponentReflect(
  name: string,
  canonical: JsonValue,
  index: number,
  pending: JsonValue,
) {
  const current = doc.value;
  if (!current) return;
  const entity = current.entities[index];
  if (!entity) return;
  const value = entity.components[name];
  if (JSON.stringify(value) !== JSON.stringify(pending)) return;
  if (JSON.stringify(value) === JSON.stringify(canonical)) return;
  const entities = current.entities.slice();
  entities[index] = {
    ...entity,
    components: { ...entity.components, [name]: canonical },
  };
  silentUpdate({ ...current, entities });
}

// ── 组件复制 / 粘贴 ─────────────────────────────────────────────────────────

function copyComponent(name: string) {
  if (!doc.value || primary.value === null) return;
  const entity = doc.value.entities[primary.value];
  const value = entity?.components[name];
  if (value === undefined) return;
  const cloned = JSON.parse(JSON.stringify(value)) as JsonValue;
  clipboard.value = { component: name, value: cloned };
  status.value = `已复制组件 ${name.split("::").pop()}`;
}

function pasteComponent() {
  const buffer = clipboard.value;
  if (!buffer || !doc.value || !selection.value.length) return;
  const target = new Set(selection.value);
  // 先克隆一次，避免在 map 回调里做递归类型推断（TS 会报过深实例化）。
  const pasted = JSON.parse(JSON.stringify(buffer.value)) as JsonValue;
  const entities: EntityJson[] = doc.value.entities.map((entity, index) => {
    if (!target.has(index)) return entity;
    return { ...entity, components: { ...entity.components, [buffer.component]: pasted } };
  });
  commit({ ...doc.value, entities }, `paste-component:${buffer.component}#${Date.now()}`);
  status.value = `已粘贴 ${buffer.component.split("::").pop()} 到 ${target.size} 个实体`;
}

function clearClipboard() {
  clipboard.value = null;
}

// ── 增删 ────────────────────────────────────────────────────────────────────

function nextEntityId(entities: EntityJson[]): number {
  let max = -1;
  for (const e of entities) {
    if (typeof e.id === "number" && e.id > max) {
      max = e.id;
    }
  }
  return max + 1;
}

function addEntity() {
  if (!doc.value) return;
  const preset = presets.value.find((p) => p.id === presetId.value);
  const components: Record<string, JsonValue> = {};
  for (const name of preset?.components ?? []) {
    const type = schema.value.types.find((t) => t.name === name);
    const defaults = (type?.defaultJson as JsonValue) ?? {};
    // 预设可以覆盖部分字段（例如把 `Light` 的默认 Point 改成 Directional/Spot）。
    const overrides = preset?.overrides?.[name];
    components[name] =
      overrides && typeof overrides === "object" && !Array.isArray(overrides)
        ? { ...(defaults as Record<string, JsonValue>), ...(overrides as Record<string, JsonValue>) }
        : defaults;
  }
  const entities = doc.value.entities.slice();
  const id = nextEntityId(entities);
  entities.push({
    id,
    name: uniqueName(preset?.name ?? "entity"),
    parent: null,
    transform: {
      position: (preset?.position ?? [0, 0, 0]) as number[],
      rotation: (preset?.rotation ?? [0, 0, 0, 1]) as number[],
      scale: [1, 1, 1],
    },
    components,
  });
  commit({ ...doc.value, entities }, `add-entity#${entities.length}`);
  selection.value = [entities.length - 1];
}

function duplicateEntity() {
  if (!doc.value || primary.value === null) return;
  const sourceIndex = primary.value;
  const source = doc.value.entities[sourceIndex];
  if (!source) return;

  const entities = doc.value.entities.slice();
  const copy = JSON.parse(JSON.stringify(source)) as EntityJson;
  copy.id = nextEntityId(entities);
  copy.name = uniqueName(`${source.name ?? "entity"}_copy`);
  // 保持与源实体相同的父级 ID（parent 是实体稳定 id 而不是数组下标）。
  copy.parent = source.parent ?? null;

  const insertAt = sourceIndex + 1;
  entities.splice(insertAt, 0, copy);

  commit({ ...doc.value, entities }, `duplicate-entity#${insertAt}`);
  selection.value = [insertAt];
}

function duplicateEntityAt(index: number) {
  selection.value = [index];
  duplicateEntity();
}

function removeEntityAt(indices: number[]) {
  selection.value = indices.length ? [...indices] : [];
  void deleteSelection();
}

function toggleEntityActive(index: number, active: boolean) {
  if (!doc.value) return;
  const entity = doc.value.entities[index];
  if (!entity) return;
  const components = { ...entity.components };
  components["prism_engine::scene::Active"] = active;
  const entities = doc.value.entities.slice();
  entities[index] = { ...entity, components };
  commit({ ...doc.value, entities }, `toggle-active:${index}:${active}`);
}

async function deleteSelection() {
  if (!doc.value || !selection.value.length) return;
  const indices = selection.value.slice().sort((a, b) => a - b);
  if (indices.length >= doc.value.entities.length) {
    fail(t("dialog.atLeastOneEntity"));
    return;
  }
  if (editorPrefs.value.confirmDelete) {
    const ok = await showConfirm(
      t("dialog.deleteEntitiesTitle"),
      t("dialog.deleteEntitiesMsg", { count: indices.length }),
      { danger: true },
    );
    if (!ok) return;
  }
  try {
    const next = await runDocCommand(
      (snapshot) => api.deleteEntities(snapshot, indices),
      (deleted) => deleted,
      `delete-entities#${Date.now()}`,
    );
    if (!next) return;
    selection.value = [];
    clampSelection();
    error.value = "";
    status.value = `已删除 ${indices.length} 个实体`;
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------------------
// 快捷键
// ---------------------------------------------------------------------------

function isTextInput(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  if (!element) return false;
  const tag = element.tagName?.toLowerCase();
  return (
    tag === "input" || tag === "textarea" || tag === "select" || element.isContentEditable === true
  );
}

function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") {
    openMenu.value = null;
    return;
  }

  const mod = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();

  // 焦点在文本输入上时，把按键交还给文本框：`Ctrl+Z/Y`（文本撤销/重做）、
  // `Ctrl+A`（全选）、`Ctrl+V`（粘贴）、`Ctrl+D` 都应由浏览器处理。
  // 唯一保留的是保存 —— 编辑字段时按 `Ctrl+S` 仍应落盘。
  if (isTextInput(event.target)) {
    if (mod && key === "s") {
      event.preventDefault();
      void save();
    }
    return;
  }

  if (!mod && event.key === "Delete") {
    event.preventDefault();
    void deleteSelection();
    return;
  }

  if (!mod) return;
  if (key === "s") {
    event.preventDefault();
    void save();
  } else if (key === "n") {
    event.preventDefault();
    void createDocumentOf("scene");
  } else if (key === "o") {
    event.preventDefault();
    showPanelTab("docs");
  } else if (key === "d") {
    event.preventDefault();
    duplicateEntity();
  } else if (key === "v" && clipboard.value) {
    event.preventDefault();
    pasteComponent();
  } else if (key === "a") {
    if (doc.value) {
      event.preventDefault();
      selection.value = doc.value.entities.map((_, index) => index);
    }
  } else if (key === "z" && !event.shiftKey) {
    event.preventDefault();
    undo();
  } else if ((key === "z" && event.shiftKey) || key === "y") {
    event.preventDefault();
    redo();
  }
}

// ── 运行 / 预览 ─────────────────────────────────────────────────────────────

/** 读取 `assets/scenes.toml` 与运行时包状态。 */
async function refreshPlayable() {
  playableScenes.value = await api.listPlayableScenes().catch(() => []);
  packageStatus.value = await api.scenePackageStatus().catch(() => ({
    exists: false,
    sizeBytes: 0,
  }));
  if (!playableScenes.value.some((s) => s.name === playSceneName.value)) {
    playSceneName.value = playableScenes.value[0]?.name ?? "";
  }
}

/** 重新 cook + 打包所有 manifest 场景（`build-scenes`）。 */
async function buildScenes() {
  busy.value = true;
  status.value = "构建场景包…";
  try {
    logText.value = (await api.buildScenes()) || "（无输出）";
    await refreshPlayable();
    status.value = packageStatus.value.exists ? "场景包构建完成" : "构建结束（未找到 scenes.pak）";
  } catch (e) {
    fail(e);
  } finally {
    busy.value = false;
  }
}

// ── 游戏日志 ────────────────────────────────────────────────────────────────

function scrollLogsToEnd() {
  // ConsolePanel handles its own auto-scroll on log updates
}

function appendGameLog(entry: PreviewLogLine) {
  gameLogEntries.value.push(entry);
  const max = Math.max(200, editorPrefs.value.maxLogLines);
  if (gameLogEntries.value.length > max) {
    gameLogEntries.value.splice(0, gameLogEntries.value.length - max);
  }
  scrollLogsToEnd();
}

function clearAllLogs() {
  gameLogEntries.value = [];
  logText.value = "";
}

async function loadGameLogs() {
  try {
    gameLogEntries.value = await api.previewLogs();
    scrollLogsToEnd();
  } catch {
    // 未启动过预览时没有日志。
  }
}

/**
 * 「运行 / 停止」：驱动 Game view 在**编辑器进程内**模拟当前场景。
 *
 * 与旧实现（spawn `projects/prisma` 二进制 + 读 stdout）的关键差别：
 * 不再有第二个进程，场景来源是当前打开的文档（含未保存改动），不需要先
 * cook 成 `scenes.pak`。
 */
async function togglePlay() {
  if (playing.value) {
    playing.value = false;
    paused.value = false;
    status.value = "已停止";
    return;
  }
  if (!doc.value || !isScene.value) {
    fail("先打开一个场景再运行");
    return;
  }
  showPanelTab("gameView");
  paused.value = false;
  playing.value = true;
  status.value = `运行场景 ${currentPath.value ?? ""}`;
}

/** 暂停 / 继续 Game view 的模拟（场景还在渲染，方便看住某一帧）。 */
function togglePause() {
  if (!playing.value) return;
  paused.value = !paused.value;
  status.value = paused.value ? "已暂停（可单步）" : "继续运行";
}

/** 暂停中前进一帧（真正的 `editor_viewport_step` 由 Game 面板按自己的 id 下发）。 */
function stepGame() {
  if (!playing.value) return;
  paused.value = true;
  status.value = "单步前进一帧";
}

// ── 项目设置 / 首选项 / 构建 ────────────────────────────────────────────────

/** 设置类窗口默认以较大的浮动面板打开；用户仍可拖回 dock 停靠。 */
function openUtilityWindow(id: PanelId, width = 760, height = 640) {
  if (isPanelVisible(id)) {
    showPanelTab(id);
    return;
  }
  const z = floatingPanels.value.reduce((max, panel) => Math.max(max, panel.z), 9000) + 1;
  floatingPanels.value = [
    ...floatingPanels.value,
    {
      tab: id,
      x: Math.max(24, Math.round((window.innerWidth - width) / 2)),
      y: Math.max(48, Math.round((window.innerHeight - height) / 2)),
      width,
      height,
      z,
    },
  ];
}

function openSettings() {
  openUtilityWindow("projectSettings");
}

function openPreferences() {
  openUtilityWindow("preferences", 640, 560);
}

function openTheme() {
  openUtilityWindow("theme", 720, 640);
}

function openBuild() {
  openUtilityWindow("build", 760, 620);
}

// ── 离线工具 ────────────────────────────────────────────────────────────────

function openTools(tab?: "gi" | "heightmap") {
  if (tab) toolTab.value = tab;
  showPanelTab("tools");
  toolLog.value = "";
}

// ── 顶部菜单栏 ──────────────────────────────────────────────────────────────

interface MenuItem {
  label?: string;
  shortcut?: string;
  separator?: boolean;
  disabled?: () => boolean;
  checked?: () => boolean;
  children?: MenuItem[];
  action?: () => void;
}

interface MenuDef {
  id: string;
  label: string;
  items: MenuItem[];
}

function toggleMenu(id: string) {
  openMenu.value = openMenu.value === id ? null : id;
}

/** 菜单已展开时，鼠标移到相邻菜单直接切换（UE / Unity 行为）。 */
function hoverMenu(id: string) {
  if (openMenu.value) openMenu.value = id;
}

/** 点击菜单栏空白处 / 右侧按钮时收起菜单。 */
function onMenubarClick(event: MouseEvent) {
  const target = event.target as HTMLElement | null;
  if (!target?.closest(".menu-root")) openMenu.value = null;
}

function runMenu(item: MenuItem) {
  if (item.disabled?.()) return;
  openMenu.value = null;
  item.action?.();
}

const MENUS = computed<MenuDef[]>(() => {
  const newTemplateItems: MenuItem[] = [
    ...sceneTemplates.value.map((template) => ({
      label: `场景：${template.label}`,
      action: () => void createDocumentWithTemplate("scene", template.id),
    })),
    { separator: true },
    ...presets.value.map((preset) => ({
      label: `预制体：${preset.label}`,
      action: () => void createDocumentWithTemplate("prefab", preset.id),
    })),
  ];

  return [
    {
      id: "file",
      label: "文件",
      items: [
        { label: "新建场景", shortcut: "Ctrl+N", action: () => void createDocumentOf("scene") },
        { label: "新建预制体", action: () => void createDocumentOf("prefab") },
        { label: "按模板新建", children: newTemplateItems },
        { label: "打开文档", shortcut: "Ctrl+O", action: () => showPanelTab("docs") },
        { separator: true },
        {
          label: "保存",
          shortcut: "Ctrl+S",
          disabled: () => !canSave.value || busy.value,
          action: () => void save(),
        },
        {
          label: "另存为…",
          disabled: () => !hasDoc.value || busy.value,
          action: () => void saveAs(),
        },
        {
          label: "重命名…",
          disabled: () => !hasPath.value || busy.value,
          action: () => void renameCurrent(),
        },
        {
          label: "删除…",
          disabled: () => !hasPath.value || busy.value,
          action: () => void deleteCurrent(),
        },
        { separator: true },
        {
          label: "重新加载当前文档",
          disabled: () => !hasPath.value || busy.value,
          action: () => void reload(),
        },
      ],
    },
    {
      id: "edit",
      label: "编辑",
      items: [
        { label: "撤销", shortcut: "Ctrl+Z", disabled: () => !canUndo.value, action: undo },
        {
          label: "重做",
          shortcut: "Ctrl+Shift+Z",
          disabled: () => !canRedo.value,
          action: redo,
        },
        { separator: true },
        {
          label: "复制选中实体",
          shortcut: "Ctrl+D",
          disabled: () => !selectedEntity.value,
          action: duplicateEntity,
        },
        {
          label: "删除选中实体",
          shortcut: "Del",
          disabled: () => !selectedEntity.value,
          action: () => void deleteSelection(),
        },
        {
          label: "全选实体",
          shortcut: "Ctrl+A",
          disabled: () => !doc.value,
          action: () => {
            if (doc.value) selection.value = doc.value.entities.map((_, index) => index);
          },
        },
        { separator: true },
        {
          label: "粘贴组件",
          shortcut: "Ctrl+V",
          disabled: () => !clipboard.value || !selectedEntity.value,
          action: pasteComponent,
        },
        {
          label: "清空组件剪贴板",
          disabled: () => !clipboard.value,
          action: clearClipboard,
        },
        { separator: true },
        { label: "项目设置…", action: openSettings },
        { label: "首选项…", action: openPreferences },
        { label: "主题…", action: openTheme },
      ],
    },
    {
      id: "build",
      label: "构建",
      items: [
        { label: "构建项目…", disabled: () => busy.value, action: openBuild },
        { label: "构建场景包", disabled: () => busy.value, action: () => void buildScenes() },
        {
          label: "Cook 当前场景",
          disabled: () => !hasPath.value || !isScene.value || busy.value,
          action: () => void cook(),
        },
        { separator: true },
        {
          label: "离线工具",
          children: [
            { label: "GI 烘焙…", action: () => openTools("gi") },
            { label: "高度图生成…", action: () => openTools("heightmap") },
          ],
        },
      ],
    },
    {
      id: "window",
      label: "窗口",
      items: [
        ...panelMenuRoots.value.window,
        { separator: true },
        { label: "重置布局", action: resetLayout },
      ],
    },
    {
      id: "help",
      label: "帮助",
      items: panelMenuRoots.value.help,
    },
  ];
});

const SHORTCUTS: Array<{ label: string; keys: string }> = [
  { label: "保存当前文档", keys: "Ctrl+S" },
  { label: "撤销 / 重做", keys: "Ctrl+Z / Ctrl+Shift+Z" },
  { label: "复制选中实体", keys: "Ctrl+D" },
  { label: "删除选中实体", keys: "Delete" },
  { label: "全选实体", keys: "Ctrl+A" },
  { label: "粘贴组件", keys: "Ctrl+V" },
  { label: "新建场景", keys: "Ctrl+N" },
  { label: "打开文档面板", keys: "Ctrl+O" },
  { label: "关闭菜单 / 弹窗", keys: "Esc" },
  { label: "拖拽标签：合并 / 分屏", keys: "拖到面板中央 / 边缘" },
  { label: "拖拽标签：浮动窗口", keys: "拖出面板区 或按住 Alt" },
  { label: "停靠到主窗口三边", keys: "拖到左 / 右 / 底边" },
];

async function runBakeGi() {
  if (toolRunning.value) return;
  toolRunning.value = true;
  toolLog.value = "正在烘焙 GI…（首次编译 / 大场景可能较慢）";
  try {
    toolLog.value = await api.bakeGi({ ...giForm });
    status.value = "GI 烘焙完成";
  } catch (e) {
    toolLog.value = `失败：${e instanceof Error ? e.message : String(e)}`;
    fail(e);
  } finally {
    toolRunning.value = false;
  }
}

async function runHeightmap() {
  if (toolRunning.value) return;
  toolRunning.value = true;
  toolLog.value = "正在生成高度图…";
  try {
    toolLog.value = await api.generateHeightmap({ ...heightForm });
    status.value = "高度图生成完成";
  } catch (e) {
    toolLog.value = `失败：${e instanceof Error ? e.message : String(e)}`;
    fail(e);
  } finally {
    toolRunning.value = false;
  }
}

// ── 渲染图 ─────────────────────────────────────────────────────────────────

function passColor(kind: string): string {
  return PASS_COLORS[kind] ?? PASS_COLORS.unknown;
}

function resourceColor(kind: string): string {
  return RESOURCE_COLORS[kind] ?? RESOURCE_COLORS.other;
}

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString();
}

async function refreshGraph() {
  try {
    graphReport.value = await api.renderGraphReport();
    graphError.value = "";
  } catch (e) {
    graphError.value = e instanceof Error ? e.message : String(e);
  }
}

function startGraphPolling() {
  stopGraphPolling();
  graphTimer = window.setInterval(refreshGraph, 1500);
}

function stopGraphPolling() {
  if (graphTimer !== undefined) {
    window.clearInterval(graphTimer);
    graphTimer = undefined;
  }
}

/**
 * 把 pass 列表排成横向执行顺序，并按「谁生产这个资源」连边。
 * 只做纯布局，SVG 由模板渲染。
 */
const graphLayout = computed(() => {
  const report = graphReport.value;
  if (!report || !report.passes.length) {
    return { nodes: [], edges: [], width: 720, height: GRAPH_HEIGHT };
  }
  const passes = [...report.passes].sort((a, b) => a.index - b.index);
  const resources = new Map(report.resources.map((r) => [r.handle, r]));
  const xOf = (i: number) => 24 + i * (GRAPH_NODE_W + GRAPH_GAP);

  const nodes = passes.map((pass, i) => ({ pass, x: xOf(i), y: GRAPH_TOP }));
  const producer = new Map<number, number>();
  const seen = new Set<string>();
  const edges: {
    key: string;
    path: string;
    color: string;
    label: string;
    labelX: number;
    labelY: number;
  }[] = [];

  passes.forEach((pass, i) => {
    pass.outputs.forEach((handle) => producer.set(handle, i));
    pass.inputs.forEach((handle) => {
      const from = producer.get(handle);
      if (from === undefined || from >= i) return;
      const key = `${from}->${i}:${handle}`;
      if (seen.has(key)) return;
      seen.add(key);

      const offset = edges.filter((e) => e.key.startsWith(`${from}->${i}:`)).length * 11;
      const y = GRAPH_TOP + GRAPH_NODE_H / 2 + offset;
      const x1 = xOf(from) + GRAPH_NODE_W;
      const x2 = xOf(i);
      const mx = (x1 + x2) / 2;
      const resource = resources.get(handle);
      edges.push({
        key,
        path: `M ${x1} ${y} C ${mx} ${y}, ${mx} ${y}, ${x2} ${y}`,
        color: resourceColor(resource?.kind ?? "other"),
        label: resource?.name ?? `#${handle}`,
        labelX: mx,
        labelY: y - 6,
      });
    });
  });

  const width = 24 + passes.length * (GRAPH_NODE_W + GRAPH_GAP) - GRAPH_GAP + 24;
  return { nodes, edges, width: Math.max(width, 720), height: GRAPH_HEIGHT };
});

let unlistenPreviewLog: (() => void) | undefined;
let autoSaveTimer: number | undefined;

function scheduleAutoSave(preferences: EditorPreferences) {
  if (autoSaveTimer !== undefined) window.clearInterval(autoSaveTimer);
  autoSaveTimer = undefined;
  if (preferences.autoSaveSeconds <= 0) return;
  autoSaveTimer = window.setInterval(() => {
    if (!dirty.value || !currentPath.value || busy.value) return;
    void save();
  }, preferences.autoSaveSeconds * 1000);
}

function applyPreferences(preferences: EditorPreferences) {
  document.documentElement.classList.toggle("compact", preferences.compactMode);
  if (
    preferences.defaultPlayScene &&
    playableScenes.value.some((scene) => scene.name === preferences.defaultPlayScene)
  ) {
    playSceneName.value = preferences.defaultPlayScene;
  }
  scheduleAutoSave(preferences);
}

watch(editorPrefs, (value) => {
  savePreferences(value);
  applyPreferences(value);
}, { deep: true, immediate: true });

watch(themeState, (value) => {
  applyTheme(value);
  saveThemeState(value);
}, { deep: true, immediate: true });

watch(playableScenes, (scenes) => {
  if (
    editorPrefs.value.defaultPlayScene &&
    scenes.some((scene) => scene.name === editorPrefs.value.defaultPlayScene)
  ) {
    playSceneName.value = editorPrefs.value.defaultPlayScene;
  }
});

onMounted(() => {
  void bootstrap();
  window.addEventListener("keydown", onKey);
  loadLayout();
  void loadWindowRegistry();
  void loadGameLogs();
  void api.onPreviewLog(appendGameLog).then((unlisten) => {
    unlistenPreviewLog = unlisten;
  });
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
  unlistenPreviewLog?.();
  if (autoSaveTimer !== undefined) window.clearInterval(autoSaveTimer);
  stopGraphPolling();
});
</script>

<template>
  <div class="app">
    <header class="menubar" @click="onMenubarClick">
      <nav class="menubar-items">
        <div v-for="menu in MENUS" :key="menu.id" class="menu-root">
          <button
            class="menu-trigger"
            :class="{ open: openMenu === menu.id }"
            @click.stop="toggleMenu(menu.id)"
            @mouseenter="hoverMenu(menu.id)"
          >
            {{ menu.label }}
          </button>
          <div v-if="openMenu === menu.id" class="menu-popup" @click.stop>
            <template v-for="(item, index) in menu.items" :key="index">
              <div v-if="item.separator" class="menu-separator" />
              <div v-else-if="item.children" class="menu-submenu">
                <button
                  class="menu-item menu-submenu-trigger"
                  :disabled="item.disabled?.() ?? false"
                >
                  <span v-if="item.checked" class="menu-check">
                    <Check v-if="item.checked()" :size="12" />
                  </span>
                  <span class="menu-item-label">{{ item.label }}</span>
                  <ChevronRight class="menu-submenu-arrow" :size="12" />
                </button>
                <div class="menu-popup menu-submenu-popup">
                  <template v-for="(child, childIndex) in item.children" :key="childIndex">
                    <div v-if="child.separator" class="menu-separator" />
                    <button
                      v-else
                      class="menu-item"
                      :disabled="child.disabled?.() ?? false"
                      @click.stop="runMenu(child)"
                    >
                      <span v-if="child.checked" class="menu-check">
                        <Check v-if="child.checked()" :size="12" />
                      </span>
                      <span class="menu-item-label">{{ child.label }}</span>
                      <span class="menu-shortcut">{{ child.shortcut ?? "" }}</span>
                    </button>
                  </template>
                </div>
              </div>
              <button
                v-else
                class="menu-item"
                :disabled="item.disabled?.() ?? false"
                @click="runMenu(item)"
              >
                <span v-if="item.checked" class="menu-check">
                  <Check v-if="item.checked()" :size="12" />
                </span>
                <span class="menu-item-label">{{ item.label }}</span>
                <span class="menu-shortcut">{{ item.shortcut ?? "" }}</span>
              </button>
            </template>
          </div>
        </div>
      </nav>

    </header>

    <div v-if="openMenu" class="menu-backdrop" @click="openMenu = null" @contextmenu.prevent />

    <div class="main-toolbar">
      <!-- 左侧：变换工具组与坐标空间切换 -->
      <div class="toolbar-section toolbar-left">
        <div class="segmented-group" :title="$t('toolbar.transformTools')">
          <button
            class="segmented-btn"
            :class="{ active: activeTool === 'select' }"
            :title="$t('toolbar.toolSelect')"
            @click="activeTool = 'select'"
          >
            <MousePointer :size="13" />
          </button>
          <button
            class="segmented-btn"
            :class="{ active: activeTool === 'move' }"
            :title="$t('toolbar.toolMove')"
            @click="activeTool = 'move'"
          >
            <Move :size="13" />
          </button>
          <button
            class="segmented-btn"
            :class="{ active: activeTool === 'rotate' }"
            :title="$t('toolbar.toolRotate')"
            @click="activeTool = 'rotate'"
          >
            <RotateCw :size="13" />
          </button>
          <button
            class="segmented-btn"
            :class="{ active: activeTool === 'scale' }"
            :title="$t('toolbar.toolScale')"
            @click="activeTool = 'scale'"
          >
            <Maximize2 :size="13" />
          </button>
        </div>

        <div class="segmented-group">
          <button
            class="segmented-btn"
            :title="$t('toolbar.pivotTooltip')"
            @click="activePivot = activePivot === 'pivot' ? 'center' : 'pivot'"
          >
            {{ activePivot === "pivot" ? $t("toolbar.pivot") : $t("toolbar.center") }}
          </button>
          <button
            class="segmented-btn"
            :title="$t('toolbar.spaceTooltip')"
            @click="activeSpace = activeSpace === 'global' ? 'local' : 'global'"
          >
            {{ activeSpace === "global" ? $t("toolbar.global") : $t("toolbar.local") }}
          </button>
        </div>
      </div>

      <!-- 中间：Unity 经典对称三段式 Play / Pause / Step 运行控制 -->
      <div class="toolbar-section toolbar-center">
        <div class="segmented-group">
          <button
            class="segmented-btn play-btn"
            :class="{ running: playing }"
            :disabled="!hasDoc || !isScene"
            :title="$t('toolbar.playTooltip')"
            @click="togglePlay"
          >
            <Square v-if="playing" :size="12" fill="currentColor" />
            <Play v-else :size="13" fill="currentColor" />
          </button>
          <button
            class="segmented-btn pause-btn"
            :class="{ paused }"
            :disabled="!playing"
            :title="$t('toolbar.pauseTooltip')"
            @click="togglePause"
          >
            <Pause :size="13" fill="currentColor" />
          </button>
          <button
            class="segmented-btn step-btn"
            :disabled="!playing"
            :title="$t('toolbar.stepTooltip')"
            @click="stepGame"
          >
            <StepForward :size="13" />
          </button>
        </div>
      </div>

      <!-- 右侧：当前场景与布局切换 -->
      <div class="toolbar-section toolbar-right">
        <EditorDropdown
          v-if="scenes.length"
          :model-value="currentPath ?? ''"
          :options="sceneDropdownOptions"
          :title="$t('toolbar.switchScene')"
          @update:model-value="(val) => onToolbarSceneSelect(String(val))"
        />
        <span v-else class="tool-select scene-select viewport-scene-label">
          {{ documentLabel }}
        </span>

        <EditorDropdown
          :model-value="activeLayout"
          :options="layoutDropdownOptions"
          :title="$t('toolbar.layoutPresets')"
          @update:model-value="(val) => applyLayout(String(val))"
        />
      </div>
    </div>

    <div v-if="error" class="error">{{ error }}</div>
    <div v-else-if="validationError" class="warn">校验未通过：{{ validationError }}</div>

    <div class="workspace">
      <DockLayout
        v-model:layout="dockLayout"
        v-model:floating="floatingPanels"
        :panels="PANELS"
      >
        <template #sceneView>
          <ViewportPanel
            kind="scene"
            :doc="doc"
            :path="currentPath"
            :selection="selection"
            @select="onViewportSelect"
            @move="onViewportMove"
          />
        </template>
        <template #gameView>
          <ViewportPanel
            kind="game"
            :doc="doc"
            :path="currentPath"
            :playing="playing"
            :paused="paused"
            @toggle-play="togglePlay"
            @toggle-pause="togglePause"
            @step="stepGame"
          />
        </template>
        <template #docs>
        <div class="section-body">
          <div class="group">场景</div>
          <div
            v-for="s in scenes"
            :key="s.path"
            class="list-item"
            :class="{ active: s.path === currentPath }"
            @click="openDocument(s, 'scene')"
          >
            <span>{{ s.name }}</span>
            <span v-if="s.path === currentPath && dirty" class="muted" title="未保存">•</span>
          </div>
          <div v-if="!scenes.length" class="hint" style="padding: 2px 8px">（无场景）</div>

          <div class="group">预制体</div>
          <div
            v-for="p in prefabs"
            :key="p.path"
            class="list-item"
            :class="{ active: p.path === currentPath }"
            @click="openDocument(p, 'prefab')"
          >
            <span class="prefab-name">{{ p.name }}</span>
            <button
              v-if="instanceCountFor(p)"
              class="ghost icon"
              :title="`用预制体当前内容覆盖本场景的 ${instanceCountFor(p)} 个实例`"
              :disabled="busy"
              @click.stop="syncPrefab(p)"
            >
              同步 {{ instanceCountFor(p) }}
            </button>
            <button
              class="ghost icon"
              title="实例化到当前场景（挂在选中实体下）"
              :disabled="!hasDoc || !isScene || busy"
              @click.stop="insertPrefab(p)"
            >
              插入
            </button>
          </div>
          <div v-if="!prefabs.length" class="hint" style="padding: 2px 8px">
            （无预制体：把层级里的实体拖到资产目录即可保存为预制体）
          </div>
        </div>

        </template>

        <template #hierarchy>
        <div class="section-body">
          <HierarchyTree
            v-if="doc"
            :entities="doc.entities"
            :selected="selection"
            :presets="presets"
            :preset-id="presetId"
            :can-add="hasDoc"
            :scene-name="documentLabel"
            :dirty="dirty"
            @preset="presetId = $event"
            @add="addEntity"
            @select="onSelect"
            @rename="onRenameEntity"
            @reparent="onReparent"
            @move="onMove"
            @duplicate="duplicateEntityAt"
            @remove="removeEntityAt"
            @toggle-active="toggleEntityActive"
            @insert-prefab="insertPrefabAt"
          />
        </div>

        </template>

        <template #assets>
        <AssetBrowser
          :before-change="ensureSavedForAssetOp"
          @select="onAssetSelect"
          @jump="jumpToUsage"
          @changed="onAssetsChanged"
          @open="openAssetPath"
          @prefab-drop="createPrefabAt"
        />

        </template>

        <template #inspector>
        <div class="editor-pane inspector-pane">
          <!-- 选中资产时展示 Unity 风格 Asset Inspector（含大图/文本预览与导入元数据） -->
          <AssetInspector
            v-if="selectedAsset"
            :asset="selectedAsset"
            @jump="jumpToUsage"
            @open="openAssetPath"
          />
          <!-- 选中实体时展示 GameObject 检查器 -->
          <Inspector
            v-else-if="selectedEntity && doc && primary !== null"
            :entity="selectedEntity"
            :index="primary"
            :entities="doc.entities"
            :schema="schema"
            :selection-count="selection.length"
            :clipboard="clipboard"
            :prefab-link="prefabLink"
            :highlight-component="highlightComponent"
            :highlight-field="highlightField"
            @update="onEntityUpdate"
            @revert-prefab="revertPrefab"
            @apply-prefab="applyPrefab"
            @unlink-prefab="unlinkPrefab"
            @copy-component="copyComponent"
            @paste-component="pasteComponent"
            @clear-clipboard="clearClipboard"
            @reflect="onComponentReflect"
            @rejected="fail"
          />
          <div v-else class="empty">{{ $t("inspector.emptyState") }}</div>
        </div>

        </template>

        <template #log>
          <ConsolePanel
            :editor-logs="logText"
            :game-logs="gameLogEntries"
            :max-log-lines="editorPrefs.maxLogLines"
            :playing="playing"
            @clear="clearAllLogs"
            @pause-game="paused = true"
          />
        </template>

        <template #projectSettings>
        <ProjectSettingsPanel :project-root="projectRoot" />
        </template>

        <template #preferences>
        <PreferencesPanel
          v-model="editorPrefs"
          :playable-scenes="playableScenes.map((scene) => scene.name)"
        />
        </template>

        <template #theme>
        <ThemePanel v-model="themeState" />
        </template>

        <template #build>
        <BuildPanel
          :scenes="scenes"
          :project-root="projectRoot"
          @built="refreshPlayable"
        />
        </template>

        <template #renderGraph>
        <div class="panel-pane graph-panel">
          <div class="graph-toolbar">
            <span v-if="graphReport" class="hint">
              frame {{ graphReport.frame }} · {{ fmtTime(graphReport.generatedAtMs) }}
            </span>
            <span class="spacer" />
            <button class="ghost icon" title="立即刷新" @click="refreshGraph">
              <RefreshCw :size="14" />
            </button>
          </div>

          <div v-if="graphReport" class="graph-toolbar">
            <span class="chip">
              {{ graphReport.settings.renderMode === "path_trace" ? "Path Tracing" : "Raster" }}
            </span>
            <span class="chip">
              Tonemap: {{ graphReport.settings.tonemapMode === 0 ? "Reinhard" : "ACES" }}
            </span>
            <span class="chip">Debug flags: 0x{{ graphReport.settings.debugFlags.toString(16) }}</span>
            <span class="chip">PT bounces: {{ graphReport.settings.ptMaxBounces }}</span>
            <span class="chip">PT samples: {{ graphReport.settings.ptMaxIterations || "∞" }}</span>
          </div>

          <div v-if="graphError" class="error" style="margin: 10px">{{ graphError }}</div>

          <div v-if="!graphReport" class="empty">
            还没有渲染图报告。
            <div class="hint" style="margin-top: 8px">
              点工具栏「运行」启动游戏预览后，渲染线程会把图结构写入报告文件，这里每秒自动刷新。
            </div>
          </div>

          <template v-else>
            <div class="graph-scroll">
              <svg
                :viewBox="`0 0 ${graphLayout.width} ${graphLayout.height}`"
                :width="graphLayout.width"
                :height="graphLayout.height"
              >
                <defs>
                  <marker
                    id="rg-arrow"
                    markerWidth="7"
                    markerHeight="7"
                    refX="6"
                    refY="3.5"
                    orient="auto"
                  >
                    <path d="M0,0 L7,3.5 L0,7 z" fill="var(--muted-dim)" />
                  </marker>
                </defs>
                <g v-for="edge in graphLayout.edges" :key="edge.key">
                  <path
                    :d="edge.path"
                    fill="none"
                    :stroke="edge.color"
                    stroke-width="1.6"
                    marker-end="url(#rg-arrow)"
                  />
                  <text
                    :x="edge.labelX"
                    :y="edge.labelY"
                    text-anchor="middle"
                    :fill="edge.color"
                    font-size="10"
                  >
                    {{ edge.label }}
                  </text>
                </g>
                <g v-for="node in graphLayout.nodes" :key="node.pass.index">
                  <rect
                    :x="node.x"
                    :y="node.y"
                    :width="158"
                    :height="64"
                    rx="8"
                    fill="var(--panel-alt)"
                    :stroke="passColor(node.pass.kind)"
                    stroke-width="1.5"
                  />
                  <text
                    :x="node.x + 12"
                    :y="node.y + 24"
                    fill="var(--text)"
                    font-size="12"
                    font-weight="600"
                  >
                    {{ node.pass.name }}
                  </text>
                  <text
                    :x="node.x + 12"
                    :y="node.y + 42"
                    :fill="passColor(node.pass.kind)"
                    font-size="10"
                  >
                    {{ node.pass.kind }} · #{{ node.pass.index }}
                  </text>
                  <text :x="node.x + 12" :y="node.y + 57" fill="var(--muted)" font-size="9">
                    {{ node.pass.inputs.length }} in · {{ node.pass.outputs.length }} out
                  </text>
                </g>
              </svg>
            </div>

            <div class="graph-resources">
              <div
                v-for="resource in graphReport.resources"
                :key="resource.handle"
                class="resource-card"
              >
                <span class="dot" :style="{ background: resourceColor(resource.kind) }" />
                <span class="name">{{ resource.name }}</span>
                <span class="hint">
                  {{ resource.kind }}
                  <template v-if="resource.format"> · {{ resource.format }}</template>
                  <template v-if="resource.width"> · {{ resource.width }}×{{ resource.height }}</template>
                  <template v-if="!resource.allocated"> · 未分配</template>
                </span>
              </div>
              <div v-if="!graphReport.resources.length" class="hint">（无图管理资源）</div>
            </div>
          </template>
        </div>
        </template>

        <template #tools>
        <div class="panel-pane tools-panel">
          <div class="tool-tabs">
            <button :class="{ active: toolTab === 'gi' }" @click="toolTab = 'gi'">GI 烘焙</button>
            <button :class="{ active: toolTab === 'heightmap' }" @click="toolTab = 'heightmap'">
              高度图
            </button>
          </div>

          <div v-if="toolTab === 'gi'" class="settings-body panel-scroll">
            <label>
              <span>每 probe 射线数</span>
              <input v-model.number="giForm.rays" type="number" min="1" max="4096" />
            </label>
            <label>
              <span>最大弹射</span>
              <input v-model.number="giForm.bounces" type="number" min="1" max="32" />
            </label>
            <label>
              <span>输出 .bin</span>
              <input v-model="giForm.output" spellcheck="false" />
            </label>
            <label>
              <span>场景包 .pak</span>
              <input v-model="giForm.pak" spellcheck="false" />
            </label>
            <label>
              <span>cooked .rscn</span>
              <input v-model="giForm.rscn" spellcheck="false" />
            </label>
            <p class="hint">
              调用 <code>prism-build-pipeline bake-gi</code>，需要在桌面 GPU 上运行；结果写入
              <code>assets/gi/probe_volume.bin</code>。
            </p>
          </div>

          <div v-else class="settings-body panel-scroll">
            <label>
              <span>输出 .raw</span>
              <input v-model="heightForm.output" spellcheck="false" />
            </label>
            <label>
              <span>宽度</span>
              <input v-model.number="heightForm.width" type="number" min="16" max="8192" />
            </label>
            <label>
              <span>高度</span>
              <input v-model.number="heightForm.height" type="number" min="16" max="8192" />
            </label>
            <label>
              <span>种子</span>
              <input v-model.number="heightForm.seed" type="number" min="0" />
            </label>
            <label>
              <span>侵蚀迭代</span>
              <input v-model.number="heightForm.iterations" type="number" min="0" max="10000" />
            </label>
            <label>
              <span>水力粒子</span>
              <input v-model.number="heightForm.particles" type="number" min="0" max="10000000" />
            </label>
            <label>
              <span>海平面 (0–1)</span>
              <input v-model.number="heightForm.seaLevel" type="number" min="0" max="1" step="0.05" />
            </label>
            <p class="hint">
              调用 <code>prism-build-pipeline heightmap</code>，输出 f32 little-endian 原始高度数据。
            </p>
          </div>

          <pre v-if="toolLog" class="tool-log">{{ toolLog }}</pre>

          <footer class="panel-footer">
            <span class="spacer" />
            <button
              class="primary"
              :disabled="toolRunning"
              @click="toolTab === 'gi' ? runBakeGi() : runHeightmap()"
            >
              {{ toolRunning ? "运行中…" : "运行" }}
            </button>
          </footer>
        </div>
        </template>

        <template #shortcuts>
        <div class="panel-pane help-pane">
          <div class="help-body panel-scroll">
            <div v-for="item in SHORTCUTS" :key="item.label" class="shortcut-row">
              <span>{{ item.label }}</span>
              <kbd>{{ item.keys }}</kbd>
            </div>
            <p class="hint">布局保存在 localStorage 的 <code>prisma-editor.dock.v4</code>。</p>
          </div>
        </div>
        </template>

        <template #about>
        <div class="panel-pane help-pane">
          <div class="help-body about-body">
            <div class="about-mark"><Box :size="22" /></div>
            <h2>PrismaRev Editor</h2>
            <p class="hint">Tauri 2 + Vue 3 · 数据层 <code>prism-editor-api</code></p>
            <p class="hint">版本 <code>{{ engineVersionText || "—" }}</code></p>
            <p>
              场景 / 预制体编辑、split-tree docking、实时渲染图、场景包构建与运行。编辑器进程不链接引擎或
              Vulkan，Cook / 构建通过子进程调用构建流水线。
            </p>
          </div>
        </div>
        </template>

      </DockLayout>
    </div>
    <AssetPicker
      v-if="pickerState"
      :current="pickerState.current"
      @pick="resolvePicker"
      @cancel="resolvePicker(null)"
    />
    <EditorModal
      v-model="modalState.open"
      :title="modalState.title"
      :message="modalState.message"
      :has-input="modalState.hasInput"
      :default-value="modalState.defaultValue"
      :placeholder="modalState.placeholder"
      :confirm-text="modalState.confirmText"
      :cancel-text="modalState.cancelText"
      :danger="modalState.danger"
      @confirm="onModalConfirm"
      @cancel="onModalCancel"
    />

    <footer class="statusbar">
      <span class="status-led" :class="{ busy }" />
      <span class="status-text">{{ status || "就绪" }}</span>
      <span class="tool-spacer" />
      <span class="status-path" :title="projectRoot">{{ projectRoot || "未连接项目" }}</span>
      <span class="status-sep" />
      <span class="status-chip">{{ documentLabel }}</span>
      <span v-if="dirty" class="status-chip dirty">未保存</span>
      <span class="status-chip">{{ hasDoc ? (isScene ? "场景" : "预制体") : "—" }}</span>
      <span class="status-chip" :class="{ ok: packageStatus.exists }">
        {{ packageStatus.exists ? "scenes.pak 就绪" : "未打包" }}
      </span>
    </footer>
  </div>
</template>
