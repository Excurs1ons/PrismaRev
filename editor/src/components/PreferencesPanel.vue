<script setup lang="ts">
import { DEFAULT_PREFERENCES, type EditorPreferences } from "../preferences";

defineProps<{ playableScenes: string[] }>();
const prefs = defineModel<EditorPreferences>({ required: true });

function reset() {
  prefs.value = { ...DEFAULT_PREFERENCES };
}
</script>

<template>
  <div class="panel-pane settings-panel preferences-panel">
    <div class="settings-tabs">
      <strong>编辑器首选项</strong>
      <span class="spacer" />
      <span class="hint">保存在本机 localStorage</span>
    </div>
    <div class="settings-body panel-scroll">
      <div class="settings-group">
        <div class="settings-group-title">工作流</div>
        <label>
          <span>自动保存（秒，0=关闭）</span>
          <input v-model.number="prefs.autoSaveSeconds" type="number" min="0" max="600" />
        </label>
        <label class="check-row">
          <input v-model="prefs.confirmDelete" type="checkbox" />
          <span>删除实体前确认</span>
        </label>
        <label class="check-row">
          <input v-model="prefs.autoOpenLog" type="checkbox" />
          <span>运行游戏时自动打开日志</span>
        </label>
        <label>
          <span>默认运行场景</span>
          <select v-model="prefs.defaultPlayScene">
            <option value="">（沿用当前选择）</option>
            <option v-for="name in playableScenes" :key="name" :value="name">{{ name }}</option>
          </select>
        </label>
      </div>

      <div class="settings-group">
        <div class="settings-group-title">界面</div>
        <label class="check-row">
          <input v-model="prefs.compactMode" type="checkbox" />
          <span>紧凑模式（减小列表行高）</span>
        </label>
        <label>
          <span>日志最大行数</span>
          <input v-model.number="prefs.maxLogLines" type="number" min="200" max="20000" step="100" />
        </label>
      </div>

      <p class="hint">
        首选项只影响编辑器，不写入 assets/settings.toml。项目级设置请用「编辑 → 项目设置」。
      </p>
    </div>
    <footer class="panel-footer">
      <button class="ghost" @click="reset">恢复默认</button>
      <span class="spacer" />
      <span class="hint">修改立即生效</span>
    </footer>
  </div>
</template>
