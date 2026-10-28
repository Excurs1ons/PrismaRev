<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ChevronDown, ChevronUp, Plus, X } from "lucide-vue-next";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";
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
/** 面板打开 / 上次保存时的快照：保存时只把差异交给后端写入。 */
const loadedSettings = ref<ProjectSettingsData | null>(null);
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

const report = computed(() => result.value?.report ?? null);

const RESULT_LABELS: Record<string, string> = {
  succeeded: "构建成功",
  failed: "构建失败",
  cancelled: "已取消",
};

const ROLE_LABELS: Record<string, string> = {
  executable: "可执行文件",
  scenePackage: "场景包",
  settings: "设置",
  metadata: "元数据",
  other: "其它",
};

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms} ms`;
}

function fileName(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || path;
}

const availableScenes = computed(() =>
  props.scenes.filter(
    (scene) => !manifest.value.some((entry) => entry.name === scene.name),
  ),
);

const targetOptions: DropdownOption[] = [
  { value: "windows", label: "Windows Desktop" },
];

const profileOptions: DropdownOption[] = [
  { value: "release", label: "Release" },
  { value: "debug", label: "Debug" },
];

const addSceneOptions = computed<DropdownOption[]>(() => [
  { value: "", label: "选择场景…" },
  ...availableScenes.value.map((s) => ({
    value: s.name,
    label: `${s.name} (${s.fileName})`,
  })),
]);

async function load() {
  try {
    const [project, scenes, buildProfile] = await Promise.all([
      api.projectSettings(),
      api.sceneManifest(),
      api.fetchBuildProfile().catch(() => null),
    ]);
    settings.value = project;
    loadedSettings.value = JSON.parse(JSON.stringify(project)) as ProjectSettingsData;
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

function targetGroup(target: string): string {
  switch (target.trim().toLowerCase()) {
    case "windows":
    case "linux":
    case "macos":
      return "Standalone";
    case "android":
      return "Android";
    case "ios":
      return "iOS";
    case "web":
    case "webgl":
    case "wasm":
      return "WebGL";
    default:
      return "Unknown";
  }
}

/** 随配置与平台选择实时响应的 Unity `Build.Profile` 派生摘要。 */
const activeProfile = computed(() => {
  const name = settings.value?.build.profile ?? profile.value?.name ?? "release";
  const target = settings.value?.build.target ?? profile.value?.target ?? "windows";
  const dev = name.toLowerCase() === "debug";
  return {
    name,
    target,
    targetGroup: targetGroup(target),
    options: {
      development: dev,
      allowDebugging: dev,
      connectWithProfiler: false,
    },
  };
});

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

function cloneSettings(value: ProjectSettingsData): ProjectSettingsData {
  return JSON.parse(JSON.stringify(value)) as ProjectSettingsData;
}

async function runBuild(runAfter: boolean) {
  if (!settings.value || !loadedSettings.value || busy.value) return;
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
    // 只提交本面板改过的字段：项目设置面板在此期间写入的 [app]/[window]
    // 以及 [build] 的其它字段不会被这份旧快照覆盖。
    await api.writeProjectSettings(loadedSettings.value, settings.value);
    loadedSettings.value = cloneSettings(settings.value);
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
        {{ `${activeProfile.targetGroup} · ${activeProfile.target} · ${activeProfile.name}` }}
      </span>
    </div>

    <div class="settings-body panel-scroll">
      <template v-if="settings">
        <div class="settings-group">
          <div class="settings-group-title">目标</div>
          <label>
            <span>平台</span>
            <EditorDropdown
              v-model="settings.build.target"
              :options="targetOptions"
              block
            />
          </label>
          <label>
            <span>配置</span>
            <EditorDropdown
              v-model="settings.build.profile"
              :options="profileOptions"
              block
            />
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
            <EditorDropdown
              v-model="addName"
              :options="addSceneOptions"
              :disabled="!availableScenes.length"
            />
            <button :disabled="!addName" @click="addScene"><Plus :size="13" /> 添加场景</button>
            <button class="ghost" :disabled="!manifest.length" @click="saveSceneList">保存列表</button>
          </div>

          <p v-if="!scenes.length" class="hint">
            assets/scenes 下还没有 .scene.json 文档；先新建场景。
          </p>
        </div>

        <p class="hint">
          Unity <code>Build.Profile</code>：<strong>{{ activeProfile.name }}</strong> ·
          {{ activeProfile.targetGroup }} / {{ activeProfile.target }} ·
          {{ enabledCount }} 个启用场景 ·
          {{ activeProfile.options.development ? "开发构建" : "发布构建" }}
          <span v-if="activeProfile.options.allowDebugging"> · 允许调试</span>
        </p>

        <p class="hint">
          构建会执行 cargo build、build-scenes，并把 exe / scenes.pak / settings.toml
          复制到输出目录。输出目录相对项目根；也可填绝对路径。
        </p>
      </template>
      <div v-else class="empty">正在读取构建设置…</div>

      <div v-if="error" class="error">{{ error }}</div>
      <div v-if="status" class="hint">{{ status }}</div>

      <div v-if="report" class="build-report">
        <div class="build-report-head">
          <span class="build-result" :class="report.summary.result">
            {{ RESULT_LABELS[report.summary.result] ?? report.summary.result }}
          </span>
          <span>{{ formatBytes(report.summary.totalSizeBytes) }}</span>
          <span>{{ formatDuration(report.summary.totalTimeMs) }}</span>
          <span class="hint">{{ report.summary.platform }} · {{ report.summary.outputPath }}</span>
        </div>
        <div class="build-report-grid">
          <div>
            <div class="settings-group-title">阶段</div>
            <div v-for="step in report.steps" :key="step.name" class="build-report-row">
              <span>{{ step.name }}</span>
              <span class="hint">{{ formatDuration(step.durationMs) }}</span>
            </div>
          </div>
          <div>
            <div class="settings-group-title">产物</div>
            <div v-for="file in report.files" :key="file.path" class="build-report-row">
              <span :title="file.path">{{ fileName(file.path) }}</span>
              <span class="hint">{{ ROLE_LABELS[file.role] ?? file.role }} · {{ formatBytes(file.sizeBytes) }}</span>
            </div>
          </div>
        </div>
      </div>

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
