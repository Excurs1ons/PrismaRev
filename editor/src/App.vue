<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import HierarchyTree from "./components/HierarchyTree.vue";
import Inspector from "./components/Inspector.vue";
import * as api from "./api";
import type { EntityJson, SceneDoc, SceneEntry, Schema } from "./types";

/** 编辑器根组件：装配工具栏、场景/层级侧栏与检查器，并管理文档状态。 */
const schema = ref<Schema>({ types: [] });
const projectRoot = ref("");
const scenes = ref<SceneEntry[]>([]);
const currentPath = ref<string | null>(null);
const doc = ref<SceneDoc | null>(null);
const selected = ref<number | null>(null);
const dirty = ref(false);
const status = ref("");
const error = ref("");

const selectedEntity = computed<EntityJson | null>(() => {
  if (!doc.value || selected.value === null) return null;
  return doc.value.entities[selected.value] ?? null;
});

function fail(e: unknown) {
  error.value = `${e}`;
}

async function bootstrap() {
  try {
    schema.value = await api.fetchSchema();
    projectRoot.value = await api.fetchProjectRoot();
    scenes.value = await api.listScenes();
    status.value = `项目根 ${projectRoot.value}`;
  } catch (e) {
    fail(e);
  }
}

async function openScene(entry: SceneEntry) {
  if (dirty.value && !window.confirm("当前场景有未保存修改，放弃它们？")) return;
  try {
    const loaded = await api.readScene(entry.path);
    doc.value = loaded;
    currentPath.value = entry.path;
    selected.value = loaded.entities.length ? 0 : null;
    dirty.value = false;
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

async function save() {
  if (!doc.value) return;
  let path = currentPath.value;
  if (!path) {
    const name = window.prompt("保存为场景名（不含 .scene.json）", "untitled");
    if (!name) return;
    path = `${projectRoot.value}/assets/scenes/${name}.scene.json`;
  }
  try {
    await api.writeScene(path, doc.value);
    currentPath.value = path;
    dirty.value = false;
    error.value = "";
    status.value = `已保存 ${path}`;
    scenes.value = await api.listScenes();
  } catch (e) {
    fail(e);
  }
}

async function createScene() {
  if (dirty.value && !window.confirm("当前场景有未保存修改，放弃它们？")) return;
  try {
    const fresh = await api.newScene();
    doc.value = fresh;
    currentPath.value = null;
    selected.value = fresh.entities.length ? 0 : null;
    dirty.value = true;
    error.value = "";
    status.value = "新场景（未保存）";
  } catch (e) {
    fail(e);
  }
}

function updateEntity(index: number, entity: EntityJson) {
  if (!doc.value) return;
  const entities = doc.value.entities.slice();
  entities[index] = entity;
  doc.value = { ...doc.value, entities };
  dirty.value = true;
}

function onEntityUpdate(entity: EntityJson) {
  if (selected.value !== null) updateEntity(selected.value, entity);
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
  doc.value = { ...doc.value, entities };
  selected.value = entities.length - 1;
  dirty.value = true;
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
  doc.value = { ...doc.value, entities };
  selected.value = null;
  dirty.value = true;
}

function onKey(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void save();
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
      <span v-if="dirty" class="dirty-dot" title="未保存" />
      <button @click="createScene">新建</button>
      <button :disabled="!currentPath" @click="reload">重新加载</button>
      <button class="primary" :disabled="!doc" title="Ctrl/Cmd+S" @click="save">保存</button>
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
            </div>
            <div v-if="!scenes.length" class="hint" style="padding: 8px">（无场景）</div>
          </div>
        </div>
        <div class="section grow">
          <div class="section-title">
            <span>层级</span>
            <span class="spacer" />
            <button class="ghost icon" :disabled="!doc" title="新增实体" @click="addEntity">+</button>
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
  </div>
</template>
