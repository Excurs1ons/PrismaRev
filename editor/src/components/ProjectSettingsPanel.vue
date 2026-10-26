<script setup lang="ts">
import { onMounted, ref } from "vue";
import * as api from "../api";
import type { ProjectSettingsData, RenderSettingsData } from "../types";

defineProps<{ projectRoot?: string }>();

const DEBUG_FLAGS = [
  "正常渲染",
  "Direct",
  "Shadow",
  "Specular",
  "Metallic",
  "Roughness",
  "DiffuseIBL",
  "SpecularIBL",
  "MultiLight",
  "AO",
  "Emissive",
  "Transmission",
  "Translucency",
  "Anisotropy",
  "ClearCoat",
  "GI",
];

type Tab = "general" | "render" | "build";
const tab = ref<Tab>("general");
const project = ref<ProjectSettingsData | null>(null);
const render = ref<RenderSettingsData | null>(null);
/** 打开面板 / 上次保存时的快照：保存时只把差异交给后端写入。 */
const loadedProject = ref<ProjectSettingsData | null>(null);
const loadedRender = ref<RenderSettingsData | null>(null);
const busy = ref(false);
const error = ref("");
const status = ref("");

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

async function load() {
  busy.value = true;
  error.value = "";
  try {
    const [p, r] = await Promise.all([api.projectSettings(), api.renderSettings()]);
    project.value = p;
    render.value = r;
    loadedProject.value = clone(p);
    loadedRender.value = clone(r);
  } catch (e) {
    error.value = `${e}`;
  } finally {
    busy.value = false;
  }
}

async function save() {
  if (!project.value || !render.value) return;
  if (!loadedProject.value || !loadedRender.value) return;
  busy.value = true;
  error.value = "";
  status.value = "";
  try {
    // 只提交本面板真正改过的字段：别的面板（构建面板 / 渲染设置）在这期间
    // 写入的值不会被这份旧快照覆盖。
    await api.writeProjectSettings(loadedProject.value, project.value);
    await api.writeRenderSettings(loadedRender.value, render.value);
    loadedProject.value = clone(project.value);
    loadedRender.value = clone(render.value);
    status.value = "已写入 assets/settings.toml（游戏下次启动生效）";
  } catch (e) {
    error.value = `${e}`;
  } finally {
    busy.value = false;
  }
}

function debugFlagIndex(flags: number): number {
  for (let bit = 0; bit < 15; bit += 1) {
    if (flags === 1 << bit) return bit + 1;
  }
  return 0;
}

function setDebugFlag(index: number) {
  if (!render.value) return;
  render.value.debugFlags = index === 0 ? 0 : 1 << (index - 1);
}

onMounted(() => void load());
</script>

<template>
  <div class="panel-pane settings-panel project-settings">
    <div class="settings-tabs">
      <button :class="{ active: tab === 'general' }" @click="tab = 'general'">项目</button>
      <button :class="{ active: tab === 'render' }" @click="tab = 'render'">渲染</button>
      <button :class="{ active: tab === 'build' }" @click="tab = 'build'">构建</button>
      <span class="spacer" />
      <span v-if="projectRoot" class="hint">{{ projectRoot }}</span>
    </div>

    <div v-if="project && render" class="settings-body panel-scroll">
      <template v-if="tab === 'general'">
        <div class="settings-group">
          <div class="settings-group-title">应用</div>
          <label><span>公司名</span><input v-model="project.app.company" type="text" /></label>
          <label><span>产品名</span><input v-model="project.app.name" type="text" /></label>
          <label><span>版本</span><input v-model="project.app.version" type="text" /></label>
        </div>

        <div class="settings-group">
          <div class="settings-group-title">窗口</div>
          <label><span>标题</span><input v-model="project.window.title" type="text" /></label>
          <label><span>默认宽度</span><input v-model.number="project.window.width" type="number" min="320" /></label>
          <label><span>默认高度</span><input v-model.number="project.window.height" type="number" min="240" /></label>
          <label class="check-row"><input v-model="project.window.resizable" type="checkbox" /><span>允许缩放</span></label>
          <label class="check-row"><input v-model="project.window.fullscreen" type="checkbox" /><span>启动全屏</span></label>
          <label class="check-row"><input v-model="project.window.vsync" type="checkbox" /><span>垂直同步</span></label>
        </div>
      </template>

      <template v-else-if="tab === 'render'">
        <label>
          <span>渲染模式</span>
          <select v-model="render.renderMode">
            <option value="raster">光栅化（PBR）</option>
            <option value="path_trace">路径追踪</option>
          </select>
        </label>
        <label>
          <span>色调映射</span>
          <select v-model.number="render.tonemapMode">
            <option :value="0">Reinhard</option>
            <option :value="1">ACES</option>
          </select>
        </label>
        <label>
          <span>PBR 调试通道</span>
          <select :value="debugFlagIndex(render.debugFlags)" @change="setDebugFlag(Number(($event.target as HTMLSelectElement).value))">
            <option v-for="(label, index) in DEBUG_FLAGS" :key="label" :value="index">{{ label }}</option>
          </select>
        </label>
        <label>
          <span>调试 RT</span>
          <select v-model.number="render.debugRt">
            <option :value="0">HDR tonemap</option>
            <option :value="1">线性深度</option>
            <option :value="2">视图空间法线</option>
          </select>
        </label>
        <template v-if="render.renderMode === 'path_trace'">
          <label><span>最大弹射</span><input v-model.number="render.ptMaxBounces" type="number" min="1" max="64" /></label>
          <label><span>射线最大距离</span><input v-model.number="render.ptRayMaxDistance" type="number" min="1" step="10" /></label>
          <label><span>最大采样（0 = 无限）</span><input v-model.number="render.ptMaxIterations" type="number" min="0" /></label>
        </template>
        <p class="hint">写入 assets/settings.toml 的 [render] 段；运行时仍可用键盘 1–9 / Shift+1–5 / T / Tab 临时切换。</p>
      </template>

      <template v-else>
        <div class="settings-group">
          <div class="settings-group-title">目标平台</div>
          <label>
            <span>平台</span>
            <select v-model="project.build.target">
              <option value="windows">Windows Desktop</option>
            </select>
          </label>
          <label>
            <span>配置</span>
            <select v-model="project.build.profile">
              <option value="release">Release</option>
              <option value="debug">Debug</option>
            </select>
          </label>
          <label>
            <span>输出目录</span>
            <input v-model="project.build.outputDir" type="text" placeholder="build/windows" />
          </label>
          <label class="check-row"><input v-model="project.build.runAfter" type="checkbox" /><span>构建完成后运行</span></label>
        </div>
        <p class="hint">构建会执行 cargo build + 构建场景包，并把 exe、scenes.pak、settings.toml 汇总到输出目录。</p>
      </template>

      <div v-if="error" class="error">{{ error }}</div>
      <div v-if="status" class="hint">{{ status }}</div>
    </div>
    <div v-else class="empty">{{ busy ? "正在读取项目设置…" : error || "没有项目设置" }}</div>

    <footer class="panel-footer">
      <button class="ghost" :disabled="busy" @click="load">重新读取</button>
      <span class="spacer" />
      <button class="primary" :disabled="busy || !project || !render" @click="save">保存</button>
    </footer>
  </div>
</template>
