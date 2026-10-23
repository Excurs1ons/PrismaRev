<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import HierarchyTree from "./components/HierarchyTree.vue";
import Inspector from "./components/Inspector.vue";
import * as api from "./api";
import type { EntityJson, SceneDoc, SceneEntry, Schema, SceneTemplate } from "./types";

const HISTORY_LIMIT = 200;
const COALESCE_MS = 700;

const schema = ref<Schema>({ types: [] });
const projectRoot = ref("");
const scenes = ref<SceneEntry[]>([]);
const templates = ref<SceneTemplate[]>([]);
const templateId = ref("camera");

const currentPath = ref<string | null>(null);
const doc = ref<SceneDoc | null>(null);
/** 上次保存/加载时的序列化快照，用于精确判断「是否有未保存改动」。 */
const baseline = ref<string>("");

const selected = ref<number | null>(null);
const status = ref("");
const error = ref("");
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

const selectedEntity = computed<EntityJson | null>(() => {
  if (!doc.value || selected.value === null) return null;
  return doc.value.entities[selected.value] ?? null;
});

function fail(e: unknown) {
  error.value = `${e}`;
}

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

function resetHistory(loaded: SceneDoc, path: string | null) {
  past.value = [];
  future.value = [];
  doc.value = loaded;
  currentPath.value = path;
  baseline.value = JSON.stringify(loaded);
  lastKey = "";
  lastTime = 0;
  selected.value = loaded.entities.length ? 0 : null;
}

/**
 * 提交一个新文档版本。`editKey` 相同且间隔 <700ms 的连续编辑
 * （例如拖数字输入框）会合并成一次撤销。
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
  return !dirty.value || window.confirm("当前场景有未保存改动，放弃它们？");
}

// ---------------------------------------------------------------------------
// 场景文件
// ---------------------------------------------------------------------------

async function refreshScenes() {
  scenes.value = await api.listScenes();
}

async function bootstrap() {
  try {
    schema.value = await api.fetchSchema();
    projectRoot.value = await api.fetchProjectRoot();
    templates.value = await api.fetchTemplates();
    if (templates.value.length) templateId.value = templates.value[0].id;
    await refreshScenes();
    status.value = `项目根 ${projectRoot.value}`;
  } catch (e) {
    fail(e);
  }
}

async function openScene(entry: SceneEntry) {
  if (!confirmDiscard()) return;
  try {
    const loaded = await api.readScene(entry.path);
    resetHistory(loaded, entry.path);
    error.value = "";
    status.value = `已打开 ${entry.fileName}`;
  } catch (e) {
    fail(e);
  }
}

async function reload() {
  if (!currentPath.value) return;
  const entry = scenes.value.find((s) => s.path === currentPath.value);
  if (entry) await openScene(entry);
}

async function createScene() {
  if (!confirmDiscard()) return;
  try {
    const fresh = await api.newScene(templateId.value);
    resetHistory(fresh, null);
    error.value = "";
    status.value = "新场景（未保存）";
  } catch (e) {
    fail(e);
  }
}

async function save(): Promise<boolean> {
  if (!doc.value) return false;
  if (!currentPath.value) return saveAs();
  try {
    await api.writeScene(currentPath.value, doc.value);
    baseline.value = JSON.stringify(doc.value);
    error.value = "";
    status.value = `已保存 ${currentPath.value}`;
    await refreshScenes();
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function saveAs(): Promise<boolean> {
  if (!doc.value) return false;
  const name = window.prompt("保存为场景名（不含 .scene.json）", "untitled");
  if (!name) return false;
  try {
    const path = await api.writeSceneAs(name, doc.value);
    currentPath.value = path;
    baseline.value = JSON.stringify(doc.value);
    error.value = "";
    status.value = `已另存为 ${path}`;
    await refreshScenes();
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function renameCurrent() {
  if (!currentPath.value) return;
  const current = scenes.value.find((s) => s.path === currentPath.value);
  const name = window.prompt("新的场景名（不含 .scene.json）", current?.name ?? "");
  if (!name) return;
  try {
    const path = await api.renameScene(currentPath.value, name);
    currentPath.value = path;
    error.value = "";
    status.value = `已重命名为 ${path}`;
    await refreshScenes();
  } catch (e) {
    fail(e);
  }
}

async function deleteCurrent() {
  if (!currentPath.value) return;
  if (!window.confirm(`删除场景文件？\n${currentPath.value}`)) return;
  try {
    await api.deleteScene(currentPath.value);
    doc.value = null;
    currentPath.value = null;
    baseline.value = "";
    past.value = [];
    future.value = [];
    selected.value = null;
    error.value = "";
    status.value = "已删除";
    await refreshScenes();
  } catch (e) {
    fail(e);
  }
}

async function cook() {
  if (!doc.value) return;
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
// 实体编辑
// ---------------------------------------------------------------------------

function onEntityUpdate(entity: EntityJson, editKey: string) {
  if (!doc.value || selected.value === null) return;
  const entities = doc.value.entities.slice();
  entities[selected.value] = entity;
  commit({ ...doc.value, entities }, editKey);
}

function addEntity() {
  if (!doc.value) return;
  const entities = doc.value.entities.slice();
  entities.push({
    name: `entity_${entities.length}`,
    parent: null,
    transform: { translation: [0, 0, 0], rotation: [0, 0, 0, 1], scale: [1, 1, 1] },
    components: {},
  });
  commit({ ...doc.value, entities }, `add-entity#${entities.length}`);
  selected.value = entities.length - 1;
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

function onKey(event: KeyboardEvent) {
  const mod = event.ctrlKey || event.metaKey;
  if (!mod) return;
  const key = event.key.toLowerCase();
  if (key === "s") {
    event.preventDefault();
    void save();
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
      <select v-model="templateId" title="新建模板" :disabled="!templates.length">
        <option v-for="t in templates" :key="t.id" :value="t.id">{{ t.label }}</option>
      </select>
      <button @click="createScene">新建</button>
      <button :disabled="!hasPath" @click="renameCurrent">重命名</button>
      <button :disabled="!hasPath" @click="deleteCurrent">删除</button>
      <span class="sep" />
      <button :disabled="!hasDoc" @click="saveAs">另存为</button>
      <button :disabled="!hasPath || busy" title="cook 成 RSCN" @click="cook">
        {{ busy ? "cook 中…" : "Cook" }}
      </button>
      <button :disabled="!hasPath || busy" @click="reload">重新加载</button>
      <button class="primary" :disabled="!hasDoc || busy" title="Ctrl/Cmd+S" @click="save">保存</button>
    </div>

    <div v-if="error" class="error">{{ error }}</div>

    <div class="body">
      <aside class="sidebar">
        <div class="section">
          <div class="section-title">场景</div>
          <div class="section-body">
            <div
              v-for="s in scenes"
              :key="s.path"
              class="list-item"
              :class="{ active: s.path === currentPath }"
              @click="openScene(s)"
            >
              <span>{{ s.name }}</span>
              <span v-if="s.path === currentPath && dirty" class="muted" title="未保存">•</span>
            </div>
            <div v-if="!scenes.length" class="hint" style="padding: 8px">（无场景）</div>
          </div>
        </div>
        <div class="section grow">
          <div class="section-title">
            <span>层级</span>
            <span class="spacer" />
            <button class="ghost icon" :disabled="!hasDoc" title="新增实体" @click="addEntity">+</button>
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
        />
        <div v-else class="empty">打开或新建一个场景，然后在左侧选择一个实体。</div>
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
  </div>
</template>
