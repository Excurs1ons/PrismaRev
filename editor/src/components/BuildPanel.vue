<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ChevronDown, ChevronUp, Plus, X } from "lucide-vue-next";
import * as api from "../api";
import type {
  BuildProfile,
  BuildProjectResult,
  ProjectSettingsData,
  SceneEntry,
  SceneListEntry,
} from "../types";

const props = defineProps<{
  scenes: SceneEntry[];
  projectRoot: string;
}>();

const emit = defineEmits<{ (e: "built"): void }>();

const settings = ref<ProjectSettingsData | null>(null);
const manifest = ref<SceneListEntry[]>([]);
/** 后端派生的 Unity `Build.Profile`（目标组 / 构建选项 / 构建顺序）。 */
const profile = ref<BuildProfile | null>(null);
const addName = ref("");
const manifestDirty = ref(false);
const busy = ref(false);
const log = ref("");
const error = ref("");
const status = ref("");
const result = ref<BuildProjectResult | null>(null);

const availableScenes = computed(() =>
  props.scenes.filter(
    (scene) => !manifest.value.some((entry) => entry.name === scene.name),
  ),
);

async function load() {
  try {
    const [project, scenes, buildProfile] = await Promise.all([
      api.projectSettings(),
      api.sceneManifest(),
      api.fetchBuildProfile().catch(() => null),
    ]);
    settings.value = project;
    manifest.value = scenes;
    profile.value = buildProfile;
    manifestDirty.value = false;
    error.value = "";
  } catch (e) {
    error.value = `${e}`;
  }
}

/**
 * Unity 的 build index：只数**启用**的场景，所以禁用的行不占号。
 * 与后端 `BuildProfile.scenes[].buildIndex` 的规则一致。
 */
function effectiveIndex(index: number): number {
  return manifest.value.slice(0, index).filter((entry) => entry.enabled).length;
}

/** 已启用场景数（Unity `EditorBuildSettings.scenes` 里 enabled 的项）。 */
const enabledCount = computed(
  () => manifest.value.filter((entry) => entry.enabled).length,
);

function addScene() {
  const doc = props.scenes.find((scene) => scene.name === addName.value);
  if (!doc) return;
  manifest.value.push({
    name: doc.name,
    path: `scenes/${doc.fileName}`,
    enabled: true,
  });
  addName.value = "";
  manifestDirty.value = true;
  status.value = "场景已加入列表，记得保存或直接构建";
}

function removeScene(index: number) {
  manifest.value.splice(index, 1);
  manifestDirty.value = true;
  status.value = "场景已移除，记得保存或直接构建";
}

function moveScene(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= manifest.value.length) return;
  const [entry] = manifest.value.splice(index, 1);
  manifest.value.splice(to, 0, entry);
  manifestDirty.value = true;
  status.value = "顺序已调整，记得保存或直接构建";
}

async function saveSceneList() {
  try {
    await api.writeSceneManifest(manifest.value);
    manifestDirty.value = false;
    status.value = "场景列表已写入 assets/scenes.toml";
    error.value = "";
  } catch (e) {
    error.value = `${e}`;
  }
}

async function runBuild(runAfter: boolean) {
  if (!settings.value || busy.value) return;
  busy.value = true;
  error.value = "";
  status.value = "";
  result.value = null;
  log.value = runAfter ? "构建并运行…\n" : "开始构建…\n";
  try {
    if (manifestDirty.value) {
      await api.writeSceneManifest(manifest.value);
      manifestDirty.value = false;
    }
    settings.value.build.runAfter = runAfter;
    await api.writeProjectSettings(settings.value);
    const built = await api.buildProject({
      target: settings.value.build.target,
      profile: settings.value.build.profile,
      outputDir: settings.value.build.outputDir,
      runAfter,
    });
    result.value = built;
    log.value = built.output;
    status.value = `构建完成（${(built.durationMs / 1000).toFixed(1)}s）`;
    emit("built");
  } catch (e) {
    error.value = `${e}`;
    log.value = `${e}`;
  } finally {
    busy.value = false;
  }
}

async function openOutput() {
  const dir = result.value?.outputDir
    || (settings.value
      ? `${props.projectRoot}/${settings.value.build.outputDir}`.replace(/\\/g, "/")
      : "");
  if (!dir) return;
  try {
    await api.revealPath(dir);
  } catch (e) {
    error.value = `${e}`;
  }
}

onMounted(() => void load());
</script>

<template>
  <div class="panel-pane settings-panel build-panel">
    <div class="settings-tabs">
      <strong>构建项目</strong>
      <span class="spacer" />
      <span class="hint">
        {{ profile ? `${profile.targetGroup} · ${profile.target} · ${profile.name}` : "Windows Desktop" }}
      </span>
    </div>

    <div class="settings-body panel-scroll">
      <template v-if="settings">
        <div class="settings-group">
          <div class="settings-group-title">目标</div>
          <label>
            <span>平台</span>
            <select v-model="settings.build.target">
              <option value="windows">Windows Desktop</option>
            </select>
          </label>
          <label>
            <span>配置</span>
            <select v-model="settings.build.profile">
              <option value="release">Release</option>
              <option value="debug">Debug</option>
            </select>
          </label>
          <label>
            <span>输出目录</span>
            <input v-model="settings.build.outputDir" type="text" placeholder="build/windows" />
          </label>
        </div>

        <div class="settings-group">
          <div class="settings-group-title">
            场景列表（顺序 = 构建顺序，第一个启用场景为启动场景）
          </div>

          <div class="build-scene-list">
            <div v-for="(entry, index) in manifest" :key="entry.name" class="build-scene-row">
              <input
                v-model="entry.enabled"
                type="checkbox"
                :title="entry.enabled ? '参与构建' : '不参与构建'"
                @change="manifestDirty = true"
              />
              <span class="build-scene-index">{{ entry.enabled ? effectiveIndex(index) : "—" }}</span>
              <span class="build-scene-name">{{ entry.name }}</span>
              <code class="build-scene-path">{{ entry.path }}</code>
              <button class="ghost icon" title="上移" :disabled="index === 0" @click="moveScene(index, -1)">
                <ChevronUp :size="13" />
              </button>
              <button class="ghost icon" title="下移" :disabled="index === manifest.length - 1" @click="moveScene(index, 1)">
                <ChevronDown :size="13" />
              </button>
              <button class="ghost icon" title="从列表移除" @click="removeScene(index)">
                <X :size="13" />
              </button>
            </div>
            <div v-if="!manifest.length" class="hint">场景列表为空；从下面添加 assets/scenes 里的场景。</div>
          </div>

          <div class="build-add-scene">
            <select v-model="addName" :disabled="!availableScenes.length">
              <option value="">选择场景…</option>
              <option v-for="scene in availableScenes" :key="scene.path" :value="scene.name">
                {{ scene.name }}（{{ scene.fileName }}）
              </option>
            </select>
            <button :disabled="!addName" @click="addScene"><Plus :size="13" /> 添加场景</button>
            <button class="ghost" :disabled="!manifest.length" @click="saveSceneList">保存列表</button>
          </div>

          <p v-if="!scenes.length" class="hint">
            assets/scenes 下还没有 .scene.json 文档；先新建场景。
          </p>
        </div>

        <p v-if="profile" class="hint">
          Unity <code>Build.Profile</code>：<strong>{{ profile.name }}</strong> ·
          {{ profile.targetGroup }} / {{ profile.target }} ·
          {{ enabledCount }} 个启用场景 ·
          {{ profile.options.development ? "开发构建" : "发布构建" }}
          <span v-if="profile.options.allowDebugging"> · 允许调试</span>
        </p>

        <p class="hint">
          构建会执行 cargo build、build-scenes，并把 exe / scenes.pak / settings.toml
          复制到输出目录。输出目录相对项目根；也可填绝对路径。
        </p>
      </template>
      <div v-else class="empty">正在读取构建设置…</div>

      <div v-if="error" class="error">{{ error }}</div>
      <div v-if="status" class="hint">{{ status }}</div>
      <pre v-if="log" class="tool-log build-log">{{ log }}</pre>
    </div>

    <footer class="panel-footer">
      <button class="ghost" :disabled="busy" @click="openOutput">打开输出目录</button>
      <span class="spacer" />
      <button :disabled="busy || !settings" @click="runBuild(false)">
        {{ busy ? "构建中…" : "构建" }}
      </button>
      <button class="primary" :disabled="busy || !settings" @click="runBuild(true)">
        构建并运行
      </button>
    </footer>
  </div>
</template>
