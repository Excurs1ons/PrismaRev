<script setup lang="ts">
import { onMounted, ref } from "vue";
import * as api from "../api";
import type { BuildProjectResult, ProjectSettingsData } from "../types";

const props = defineProps<{
  playableScenes: string[];
  projectRoot: string;
}>();

const emit = defineEmits<{ (e: "built"): void }>();

const settings = ref<ProjectSettingsData | null>(null);
const busy = ref(false);
const log = ref("");
const error = ref("");
const result = ref<BuildProjectResult | null>(null);

async function load() {
  try {
    settings.value = await api.projectSettings();
    error.value = "";
  } catch (e) {
    error.value = `${e}`;
  }
}

async function runBuild(runAfter: boolean) {
  if (!settings.value || busy.value) return;
  busy.value = true;
  error.value = "";
  result.value = null;
  log.value = runAfter ? "构建并运行…\n" : "开始构建…\n";
  try {
    settings.value.build.runAfter = runAfter;
    await api.writeProjectSettings(settings.value);
    const built = await api.buildProject({
      target: "windows",
      profile: settings.value.build.profile,
      outputDir: settings.value.build.outputDir,
      runAfter,
    });
    result.value = built;
    log.value = built.output;
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
      <span class="hint">Windows Desktop</span>
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
          <div class="settings-group-title">场景（assets/scenes.toml）</div>
          <div v-if="playableScenes.length" class="build-scenes">
            <span v-for="name in playableScenes" :key="name" class="chip">{{ name }}</span>
          </div>
          <p v-else class="hint">没有可构建场景；先在 assets/scenes.toml 添加条目。</p>
        </div>

        <p class="hint">
          构建会执行 cargo build、build-scenes，并把 exe / scenes.pak / settings.toml
          复制到输出目录。输出目录相对项目根；也可填绝对路径。
        </p>
      </template>
      <div v-else class="empty">正在读取构建设置…</div>

      <div v-if="error" class="error">{{ error }}</div>
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
