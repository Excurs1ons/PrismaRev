<script setup lang="ts">
import { computed } from "vue";
import { Check, RotateCcw } from "lucide-vue-next";
import {
  THEME_PRESETS,
  THEME_TOKEN_GROUPS,
  isOverridden,
  presetById,
  resolveThemeColors,
  type ThemeState,
  type ThemeTokenKey,
} from "../theme";

const state = defineModel<ThemeState>({ required: true });

const colors = computed(() => resolveThemeColors(state.value));
const preset = computed(() => presetById(state.value.preset));
const overriddenCount = computed(() => Object.keys(state.value.overrides).length);

function selectPreset(id: string) {
  state.value = { preset: id, overrides: {} };
}

function setColor(key: ThemeTokenKey, value: string) {
  state.value = { preset: state.value.preset, overrides: { ...state.value.overrides, [key]: value } };
}

function resetColor(key: ThemeTokenKey) {
  const overrides = { ...state.value.overrides };
  delete overrides[key];
  state.value = { preset: state.value.preset, overrides };
}

function resetAll() {
  state.value = { preset: state.value.preset, overrides: {} };
}
</script>

<template>
  <div class="panel-pane settings-panel theme-panel">
    <div class="settings-tabs">
      <strong :title="$t('theme.storageNotice')">{{ $t("theme.title") }}</strong>
      <span class="spacer" />
      <span class="hint" :title="$t('theme.storageNotice')">{{ $t("theme.scopeNotice") }}</span>
    </div>
    <div class="settings-body panel-scroll">
      <div class="settings-group">
        <div class="settings-group-title">{{ $t("theme.presets") }}</div>
        <div class="theme-preset-grid">
          <button
            v-for="item in THEME_PRESETS"
            :key="item.id"
            class="theme-preset"
            :class="{ active: state.preset === item.id }"
            :title="item.description"
            @click="selectPreset(item.id)"
          >
            <span
              class="theme-preset-preview"
              :style="{ background: item.colors.bg, borderColor: item.colors.border }"
            >
              <span
                class="theme-preset-window"
                :style="{ background: item.colors.panel, borderColor: item.colors.border }"
              >
                <span class="theme-preset-bar" :style="{ background: item.colors.panelAlt }" />
                <span class="theme-preset-line" :style="{ background: item.colors.accent }" />
                <span
                  class="theme-preset-line short"
                  :style="{ background: item.colors.textSoft }"
                />
                <span
                  class="theme-preset-line shorter"
                  :style="{ background: item.colors.muted }"
                />
              </span>
            </span>
            <span class="theme-preset-meta">
              <strong>{{ item.label }}</strong>
            </span>
            <Check v-if="state.preset === item.id" :size="14" class="theme-preset-check" />
          </button>
        </div>
      </div>

      <div v-for="group in THEME_TOKEN_GROUPS" :key="group.id" class="settings-group">
        <div class="settings-group-title">{{ group.label }}</div>
        <div v-for="token in group.tokens" :key="token.key" class="theme-color-row">
          <span class="theme-color-label">{{ token.label }}</span>
          <input
            class="theme-color-input"
            type="color"
            :value="colors[token.key]"
            :title="`${token.label}（${token.cssVar}）`"
            @input="setColor(token.key, ($event.target as HTMLInputElement).value)"
          />
          <code class="theme-color-hex">{{ colors[token.key] }}</code>
          <button
            v-if="isOverridden(state, token.key)"
            class="ghost icon theme-color-reset"
            title="恢复预设值"
            @click="resetColor(token.key)"
          >
            <RotateCcw :size="12" />
          </button>
        </div>
      </div>

      <p class="hint">
        主题只覆盖编辑器自己的颜色变量；场景、预制体与 assets/settings.ini 都不会被写入。
      </p>
    </div>
    <footer class="panel-footer">
      <button class="ghost" :disabled="!overriddenCount" @click="resetAll">恢复预设</button>
      <span class="spacer" />
      <span class="hint">
        {{
          overriddenCount
            ? `基于「${preset.label}」自定义了 ${overriddenCount} 项颜色`
            : `跟随预设「${preset.label}」`
        }}
      </span>
    </footer>
  </div>
</template>
