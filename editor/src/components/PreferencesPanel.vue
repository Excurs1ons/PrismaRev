<script setup lang="ts">
import { computed } from "vue";
import { t, useI18n } from "../i18n";
import { DEFAULT_PREFERENCES, type EditorPreferences } from "../preferences";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";

const props = defineProps<{ playableScenes: string[] }>();
const prefs = defineModel<EditorPreferences>({ required: true });
const { locale } = useI18n();

const sceneOptions = computed<DropdownOption[]>(() => [
  { value: "", label: t("preferences.inheritCurrent") },
  ...props.playableScenes.map((s) => ({ value: s, label: s })),
]);

const localeOptions: DropdownOption[] = [
  { value: "zh-CN", label: "简体中文 (zh-CN)" },
  { value: "en-US", label: "English (en-US)" },
];

function reset() {
  prefs.value = { ...DEFAULT_PREFERENCES };
}
</script>

<template>
  <div class="panel-pane settings-panel preferences-panel">
    <div class="settings-tabs">
      <strong :title="$t('preferences.localStorageNotice')">{{ $t("preferences.title") }}</strong>
      <span class="spacer" />
      <span class="hint" :title="$t('preferences.scopeNotice')">{{ $t("preferences.scopeTooltip") }}</span>
    </div>
    <div class="settings-body panel-scroll">
      <div class="settings-group">
        <div class="settings-group-title">{{ $t("preferences.workflow") }}</div>
        <label :title="$t('preferences.autoSaveTooltip')">
          <span>{{ $t("preferences.autoSave") }}</span>
          <input v-model.number="prefs.autoSaveSeconds" type="number" min="0" max="600" />
        </label>
        <label class="check-row" :title="$t('preferences.confirmDeleteTooltip')">
          <input v-model="prefs.confirmDelete" type="checkbox" />
          <span>{{ $t("preferences.confirmDelete") }}</span>
        </label>
        <label class="check-row" :title="$t('preferences.autoOpenLogTooltip')">
          <input v-model="prefs.autoOpenLog" type="checkbox" />
          <span>{{ $t("preferences.autoOpenLog") }}</span>
        </label>
        <label :title="$t('preferences.defaultSceneTooltip')">
          <span>{{ $t("preferences.defaultPlayScene") }}</span>
          <EditorDropdown
            v-model="prefs.defaultPlayScene"
            :options="sceneOptions"
            block
          />
        </label>
      </div>

      <div class="settings-group">
        <div class="settings-group-title">{{ $t("preferences.interface") }}</div>
        <label>
          <span>{{ $t("preferences.language") }}</span>
          <EditorDropdown
            v-model="locale"
            :options="localeOptions"
            block
          />
        </label>
        <label class="check-row" :title="$t('preferences.compactModeTooltip')">
          <input v-model="prefs.compactMode" type="checkbox" />
          <span>{{ $t("preferences.compactMode") }}</span>
        </label>
        <label :title="$t('preferences.maxLogLinesTooltip')">
          <span>{{ $t("preferences.maxLogLines") }}</span>
          <input v-model.number="prefs.maxLogLines" type="number" min="200" max="20000" step="100" />
        </label>
      </div>
    </div>
    <footer class="panel-footer">
      <button class="ghost" @click="reset">{{ $t("preferences.resetDefaults") }}</button>
      <span class="spacer" />
      <span class="hint" :title="$t('preferences.autoApplyTooltip')">{{ $t("preferences.autoApply") }}</span>
    </footer>
  </div>
</template>
