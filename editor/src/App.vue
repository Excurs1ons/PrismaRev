<script setup lang="ts">
import { computed, onMounted, onUnmounted, provide, ref, watch } from "vue";
import AssetBrowser from "./components/AssetBrowser.vue";
import AssetPicker from "./components/AssetPicker.vue";
import HierarchyTree from "./components/HierarchyTree.vue";
import Inspector from "./components/Inspector.vue";
import * as api from "./api";
import { ASSETS_KEY, ASSET_PICKER_KEY } from "./keys";
import type {
  AssetInfo,
  DocKind,
  EntityJson,
  EntityPreset,
  JsonValue,
  SceneDoc,
  SceneEntry,
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
const templates = ref<SceneTemplate[]>([]);
const presets = ref<EntityPreset[]>([]);
const assets = ref<AssetInfo[]>([]);
provide(ASSETS_KEY, assets);

/** 新建时的文档种类（工具栏）。 */
const newKind = ref<DocKind>("scene");
const templateId = ref("");
const presetId = ref("empty");

/** 当前打开文档的种类。 */
const documentKind = ref<DocKind>("scene");
const currentPath = ref<string | null>(null);
const doc = ref<SceneDoc | null>(null);
/** 上次保存/加载时的序列化快照，用于精确判断「是否有未保存改动」。 */
const baseline = ref<string>("");

const selected = ref<number | null>(null);
const status = ref("");
const error = ref("");
const validationError = ref("");
const logText = ref("");
const busy = ref(false);

const past = ref<SceneDoc[]>([]);
const future = ref<SceneDoc[]>([]);
let lastKey = "";
let lastTime = 0;

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
  if (!doc.value || selected.value === null) return null;
  return doc.value.entities[selected.value] ?? null;
});

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

watch(newKind, async () => {
  templates.value = await api.documentTemplates(newKind.value);
  templateId.value = templates.value[0]?.id ?? "";
});

// ---------------------------------------------------------------------------
// 文档 / 历史
// ---------------------------------------------------------------------------

function clampSelection() {
  if (!doc.value) {
    selected.value = null;
    return;
  }
  if (selected.value !== null && selected.value >= doc.value.entities.length) {
    selected.value = doc.value.entities.length ? 0 : null;
  }
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
  selected.value = loaded.entities.length ? 0 : null;
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

function confirmDiscard(): boolean {
  return !dirty.value || window.confirm("当前文档有未保存改动，放弃它们？");
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

async function bootstrap() {
  try {
    schema.value = await api.fetchSchema();
    projectRoot.value = await api.fetchProjectRoot();
    templates.value = await api.documentTemplates("scene");
    templateId.value = templates.value[0]?.id ?? "";
    presets.value = await api.fetchPresets();
    if (presets.value.length) presetId.value = presets.value[0].id;
    await refreshDocuments();
    // 资产列表是可选增强；失败不影响编辑。
    assets.value = await api.listAssetsInfo().catch(() => []);
    status.value = `项目根 ${projectRoot.value}`;
  } catch (e) {
    fail(e);
  }
}

async function openDocument(entry: SceneEntry, kind: DocKind) {
  if (!confirmDiscard()) return;
  try {
    const loaded = await api.readDocument(entry.path);
    resetHistory(loaded, entry.path, kind);
    error.value = "";
    status.value = `已打开 ${entry.fileName}`;
  } catch (e) {
    fail(e);
  }
}

async function reload() {
  if (!currentPath.value) return;
  const list = isScene.value ? scenes.value : prefabs.value;
  const entry = list.find((s) => s.path === currentPath.value);
  if (entry) await openDocument(entry, documentKind.value);
}

async function createDocument() {
  if (!confirmDiscard()) return;
  try {
    const fresh = await api.newDocument(newKind.value, templateId.value);
    resetHistory(fresh, null, newKind.value);
    error.value = "";
    status.value = newKind.value === "scene" ? "新场景（未保存）" : "新预制体（未保存）";
  } catch (e) {
    fail(e);
  }
}

async function save(): Promise<boolean> {
  if (!doc.value) return false;
  if (!currentPath.value) return saveAs();
  try {
    await api.writeDocument(currentPath.value, doc.value);
    baseline.value = JSON.stringify(doc.value);
    error.value = "";
    status.value = `已保存 ${currentPath.value}`;
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
  const name = window.prompt(`另存为（不含 ${suffix}）`, "untitled");
  if (!name) return false;
  try {
    const path = await api.writeDocumentAs(documentKind.value, name, doc.value);
    currentPath.value = path;
    baseline.value = JSON.stringify(doc.value);
    error.value = "";
    status.value = `已另存为 ${path}`;
    await refreshDocuments();
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function renameCurrent() {
  if (!currentPath.value) return;
  const list = isScene.value ? scenes.value : prefabs.value;
  const current = list.find((s) => s.path === currentPath.value);
  const name = window.prompt("新名字（不含后缀）", current?.name ?? "");
  if (!name) return;
  try {
    const path = await api.renameDocument(currentPath.value, documentKind.value, name);
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
  if (!window.confirm(`删除文件？\n${currentPath.value}`)) return;
  try {
    await api.deleteDocument(currentPath.value);
    doc.value = null;
    currentPath.value = null;
    baseline.value = "";
    validationError.value = "";
    past.value = [];
    future.value = [];
    selected.value = null;
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

async function insertPrefab(entry: SceneEntry) {
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
    const result = await api.instantiatePrefab(doc.value, entry.path, selected.value);
    commit(result.doc, `insert-prefab:${entry.path}#${Date.now()}`);
    if (result.inserted.length) selected.value = result.inserted[0];
    error.value = "";
    status.value = `已实例化 ${entry.name}（${result.inserted.length} 个实体）`;
  } catch (e) {
    fail(e);
  } finally {
    busy.value = false;
  }
}

async function savePrefabFromEntity(index: number) {
  if (!doc.value) return;
  const entity = doc.value.entities[index];
  const name = window.prompt("预制体名（不含 .prefab.json）", entity?.name ?? "prefab");
  if (!name) return;
  try {
    const path = await api.createPrefabFromEntity(doc.value, index, name);
    await refreshDocuments();
    error.value = "";
    status.value = `已保存预制体 ${path}`;
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------------------
// 实体编辑
// ---------------------------------------------------------------------------

function uniqueName(base: string): string {
  if (!doc.value) return base;
  const names = new Set(doc.value.entities.map((entity) => entity.name));
  if (!names.has(base)) return base;
  let suffix = 2;
  while (names.has(`${base}_${suffix}`)) suffix += 1;
  return `${base}_${suffix}`;
}

function onEntityUpdate(entity: EntityJson, editKey: string) {
  if (!doc.value || selected.value === null) return;
  const entities = doc.value.entities.slice();
  entities[selected.value] = entity;
  commit({ ...doc.value, entities }, editKey);
}

/** 反射写入成功后的规范化回填（不新增撤销步）。 */
function onComponentReflect(name: string, canonical: JsonValue) {
  if (!doc.value || selected.value === null) return;
  const entity = doc.value.entities[selected.value];
  if (!entity) return;
  if (JSON.stringify(entity.components[name]) === JSON.stringify(canonical)) return;
  const entities = doc.value.entities.slice();
  entities[selected.value] = {
    ...entity,
    components: { ...entity.components, [name]: canonical },
  };
  silentUpdate({ ...doc.value, entities });
}

function addEntity() {
  if (!doc.value) return;
  const preset = presets.value.find((p) => p.id === presetId.value);
  const components: Record<string, JsonValue> = {};
  for (const name of preset?.components ?? []) {
    const type = schema.value.types.find((t) => t.name === name);
    components[name] = (type?.defaultJson as JsonValue) ?? {};
  }
  const entities = doc.value.entities.slice();
  entities.push({
    name: uniqueName(preset?.name ?? "entity"),
    parent: null,
    transform: {
      translation: (preset?.translation ?? [0, 0, 0]) as number[],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
    },
    components,
  });
  commit({ ...doc.value, entities }, `add-entity#${entities.length}`);
  selected.value = entities.length - 1;
}

function duplicateEntity() {
  if (!doc.value || selected.value === null) return;
  const source = doc.value.entities[selected.value];
  if (!source) return;

  const sourceIndex = selected.value;
  const copy = JSON.parse(JSON.stringify(source)) as EntityJson;
  copy.name = uniqueName(`${source.name ?? "entity"}_copy`);

  const insertAt = sourceIndex + 1;
  const entities = doc.value.entities.slice();
  entities.splice(insertAt, 0, copy);

  // 插入点之后的父索引整体后移一位。
  const remapped = entities.map((entity, index) => {
    if (index <= sourceIndex) return entity;
    const parent = entity.parent;
    if (parent === null || parent === undefined) return entity;
    return { ...entity, parent: parent >= insertAt ? parent + 1 : parent };
  });

  commit({ ...doc.value, entities: remapped }, `duplicate-entity#${insertAt}`);
  selected.value = insertAt;
}

function deleteEntity(index: number) {
  if (!doc.value || doc.value.entities.length <= 1) return;
  const entities = doc.value.entities
    .filter((_, i) => i !== index)
    .map((entity) => {
      const parent = entity.parent;
      if (parent === null || parent === undefined) return entity;
      if (parent === index) return { ...entity, parent: null };
      return { ...entity, parent: parent > index ? parent - 1 : parent };
    });
  commit({ ...doc.value, entities }, `delete-entity#${index}`);
  selected.value = null;
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
  const mod = event.ctrlKey || event.metaKey;

  if (!mod && event.key === "Delete" && !isTextInput(event.target)) {
    if (selected.value !== null && doc.value && doc.value.entities.length > 1) {
      event.preventDefault();
      deleteEntity(selected.value);
    }
    return;
  }

  if (!mod) return;
  const key = event.key.toLowerCase();
  if (key === "s") {
    event.preventDefault();
    void save();
  } else if (key === "d") {
    event.preventDefault();
    duplicateEntity();
  } else if (key === "z" && !event.shiftKey) {
    event.preventDefault();
    undo();
  } else if ((key === "z" && event.shiftKey) || key === "y") {
    event.preventDefault();
    redo();
  }
}

onMounted(() => {
  void bootstrap();
  window.addEventListener("keydown", onKey);
});
onUnmounted(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="app">
    <div class="toolbar">
      <span class="brand">PrismaRev Editor</span>
      <span class="status">{{ status }}</span>
      <span class="spacer" />
      <span v-if="dirty" class="dirty-dot" title="未保存改动" />
      <button class="ghost icon" :disabled="!canUndo" title="撤销 (Ctrl+Z)" @click="undo">↶</button>
      <button class="ghost icon" :disabled="!canRedo" title="重做 (Ctrl+Shift+Z)" @click="redo">↷</button>
      <span class="sep" />
      <select v-model="newKind" title="新建种类">
        <option value="scene">场景</option>
        <option value="prefab">预制体</option>
      </select>
      <select v-model="templateId" title="新建模板" :disabled="!templates.length">
        <option v-for="t in templates" :key="t.id" :value="t.id">{{ t.label }}</option>
      </select>
      <button @click="createDocument">新建</button>
      <button :disabled="!hasPath" @click="renameCurrent">重命名</button>
      <button :disabled="!hasPath" @click="deleteCurrent">删除</button>
      <span class="sep" />
      <button :disabled="!hasDoc" @click="saveAs">另存为</button>
      <button :disabled="!hasPath || !isScene || busy" title="cook 成 RSCN" @click="cook">
        {{ busy ? "cook 中…" : "Cook" }}
      </button>
      <button :disabled="!hasPath || busy" @click="reload">重新加载</button>
      <button
        class="primary"
        :disabled="!canSave || busy"
        :title="validationError ? '文档校验未通过' : 'Ctrl/Cmd+S'"
        @click="save"
      >
        保存
      </button>
    </div>

    <div v-if="error" class="error">{{ error }}</div>
    <div v-else-if="validationError" class="warn">校验未通过：{{ validationError }}</div>

    <div class="body">
      <aside class="sidebar">
        <div class="section grow">
          <div class="section-title">文档</div>
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
                class="ghost icon"
                title="实例化到当前场景（挂在选中实体下）"
                :disabled="!hasDoc || !isScene || busy"
                @click.stop="insertPrefab(p)"
              >
                插入
              </button>
            </div>
            <div v-if="!prefabs.length" class="hint" style="padding: 2px 8px">
              （无预制体：选中实体后可「存为预制体」）
            </div>
          </div>
        </div>

        <div class="section grow">
          <div class="section-title">
            <span>层级</span>
            <span class="spacer" />
            <select v-model="presetId" class="mini" title="新增实体类型">
              <option v-for="p in presets" :key="p.id" :value="p.id">{{ p.label }}</option>
            </select>
            <button
              class="ghost icon"
              :disabled="!hasDoc"
              title="按所选类型新增实体"
              @click="addEntity"
            >
              +
            </button>
          </div>
          <div class="section-body">
            <HierarchyTree
              v-if="doc"
              :entities="doc.entities"
              :selected="selected"
              @select="(i) => (selected = i)"
            />
          </div>
        </div>

        <div class="section grow">
          <div class="section-title"><span>资产</span></div>
          <div class="section-body">
            <AssetBrowser />
          </div>
        </div>
      </aside>

      <main class="editor-pane">
        <Inspector
          v-if="selectedEntity && doc && selected !== null"
          :entity="selectedEntity"
          :index="selected"
          :entities="doc.entities"
          :schema="schema"
          @update="onEntityUpdate"
          @remove="deleteEntity"
          @duplicate="duplicateEntity"
          @save-prefab="savePrefabFromEntity"
          @reflect="onComponentReflect"
          @rejected="fail"
        />
        <div v-else class="empty">打开或新建一个场景/预制体，然后在左侧选择一个实体。</div>
      </main>
    </div>

    <div v-if="logText" class="logbar">
      <div class="logbar-head">
        <span>输出</span>
        <span class="spacer" />
        <button class="ghost icon" @click="logText = ''">关闭</button>
      </div>
      <pre class="log">{{ logText }}</pre>
    </div>

    <AssetPicker
      v-if="pickerState"
      :current="pickerState.current"
      @pick="resolvePicker"
      @cancel="resolvePicker(null)"
    />
  </div>
</template>
