<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import {
  AlertCircle,
  AlertTriangle,
  Check,
  Copy,
  Info,
  Layers,
  Search,
  Trash2,
  X,
} from "lucide-vue-next";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";
import { t } from "../i18n";
import { windowRange } from "../windowed";
import type { PreviewLogLine } from "../types";

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace" | "panic";

export interface LogViewLine {
  id: string;
  source: "editor" | "game";
  level: LogLevel;
  line: string;
  stream?: string;
  timestamp?: string;
  count?: number;
}

const props = withDefaults(
  defineProps<{
    editorLogs: string;
    gameLogs: PreviewLogLine[];
    maxLogLines?: number;
    playing?: boolean;
  }>(),
  {
    maxLogLines: 5000,
    playing: false,
  },
);

const emit = defineEmits<{
  (e: "clear"): void;
  (e: "pause-game"): void;
}>();

const logScrollEl = ref<HTMLElement | null>(null);
const logContainerEl = ref<HTMLElement | null>(null);

const logSourceFilter = ref<"all" | "editor" | "game">("all");
const logLevelFilter = ref<LogLevel | "all">("all");
const searchQuery = ref("");
const isCollapsed = ref(false);
const clearOnPlay = ref(true);
const errorPause = ref(false);

const selectedLog = ref<LogViewLine | null>(null);
const copied = ref(false);

// 上下分屏高度比例（默认列表占 65%，详情占 35%）
const splitterRatio = ref(0.65);
const isDraggingSplitter = ref(false);

const logScrollTop = ref(0);
const logViewportHeight = ref(0);
const LOG_ROW_HEIGHT = 20;

function logLevelOf(line: string, stream?: string): LogLevel {
  if (/panicked at|thread '.*' panicked|\bPANIC\b|stack backtrace:/i.test(line)) {
    return "panic";
  }
  const match = line.match(/\b(ERROR|WARN(?:ING)?|INFO|DEBUG|TRACE)\b/);
  if (match) {
    const level = match[1].toUpperCase();
    if (level.startsWith("WARN")) return "warn";
    return level.toLowerCase() as LogLevel;
  }
  if (stream === "system") return "info";
  if (stream === "stderr") return "error";
  if (/失败|error/i.test(line)) return "error";
  if (/警告|warn/i.test(line)) return "warn";
  return "info";
}

const parsedEditorLogs = computed<LogViewLine[]>(() => {
  const lines = props.editorLogs.split(/\r?\n/).filter((l) => l.trim().length > 0);
  const sliced = lines.slice(-Math.max(200, props.maxLogLines));
  return sliced.map((line, idx) => ({
    id: `ed-${idx}-${line.slice(0, 16)}`,
    source: "editor",
    level: logLevelOf(line),
    line,
  }));
});

const parsedGameLogs = computed<LogViewLine[]>(() => {
  const sliced = props.gameLogs.slice(-Math.max(200, props.maxLogLines));
  return sliced.map((entry, idx) => ({
    id: `gm-${idx}-${entry.line.slice(0, 16)}`,
    source: "game",
    level: entry.level ?? logLevelOf(entry.line, entry.stream),
    line: entry.line,
    stream: entry.stream,
  }));
});

const allRawLogs = computed<LogViewLine[]>(() => {
  if (logSourceFilter.value === "editor") return parsedEditorLogs.value;
  if (logSourceFilter.value === "game") return parsedGameLogs.value;
  return [...parsedEditorLogs.value, ...parsedGameLogs.value];
});

// 三色统计数据（全部、未经过滤时的总计）
const logCounts = computed(() => {
  let info = 0;
  let warn = 0;
  let error = 0;
  for (const item of allRawLogs.value) {
    if (item.level === "info" || item.level === "debug" || item.level === "trace") info++;
    else if (item.level === "warn") warn++;
    else if (item.level === "error" || item.level === "panic") error++;
  }
  return { info, warn, error };
});

const logLevelOptions = computed<DropdownOption[]>(() => [
  { value: "all", label: t("console.levelAll") },
  { value: "error", label: t("console.levelError") },
  { value: "warn", label: t("console.levelWarn") },
  { value: "info", label: t("console.levelInfo") },
  { value: "debug", label: t("console.levelDebug") },
  { value: "trace", label: t("console.levelTrace") },
  { value: "panic", label: t("console.levelPanic") },
]);

// 过滤 + Collapse（折叠相同行）
const displayLogs = computed<LogViewLine[]>(() => {
  const q = searchQuery.value.trim().toLowerCase();
  const filtered = allRawLogs.value.filter((item) => {
    if (logLevelFilter.value !== "all" && item.level !== logLevelFilter.value) {
      return false;
    }
    if (q && !item.line.toLowerCase().includes(q)) {
      return false;
    }
    return true;
  });

  if (!isCollapsed.value) {
    return filtered;
  }

  // Unity 风格 Collapse：按 level + line 去重归类，统计出现频次
  const map = new Map<string, LogViewLine>();
  for (const item of filtered) {
    const key = `${item.level}:${item.line}`;
    const existing = map.get(key);
    if (existing) {
      existing.count = (existing.count ?? 1) + 1;
    } else {
      map.set(key, { ...item, count: 1 });
    }
  }
  return Array.from(map.values());
});

// 虚拟化列表计算
const logWindow = computed(() =>
  windowRange(
    displayLogs.value.length,
    LOG_ROW_HEIGHT,
    logScrollTop.value,
    logViewportHeight.value,
  ),
);

const visibleLogRows = computed(() =>
  displayLogs.value.slice(logWindow.value.start, logWindow.value.end),
);

function syncLogViewport() {
  const element = logScrollEl.value;
  if (!element) return;
  logScrollTop.value = element.scrollTop;
  logViewportHeight.value = element.clientHeight;
}

function scrollToBottom() {
  void nextTick(() => {
    const element = logScrollEl.value;
    if (element) {
      element.scrollTop = element.scrollHeight;
      syncLogViewport();
    }
  });
}

watch(
  () => [props.editorLogs, props.gameLogs.length],
  () => {
    scrollToBottom();
  },
);

function onLogScroll() {
  syncLogViewport();
}

function selectRow(log: LogViewLine) {
  selectedLog.value = log;
}

async function copySelected() {
  if (!selectedLog.value) return;
  try {
    await navigator.clipboard.writeText(selectedLog.value.line);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 1500);
  } catch {
    // Clipboard permission denied
  }
}

// 监听游戏运行状态
watch(
  () => props.playing,
  (nowPlaying, wasPlaying) => {
    if (nowPlaying && !wasPlaying && clearOnPlay.value) {
      emit("clear");
      selectedLog.value = null;
    }
  },
);

// 监听新日志到达，若开启了 Error Pause 并在 Play 态，自动暂停
watch(
  () => props.gameLogs.length,
  (len, prev) => {
    if (errorPause.value && props.playing && len > prev) {
      const latest = props.gameLogs[len - 1];
      const lvl = latest.level ?? logLevelOf(latest.line, latest.stream);
      if (lvl === "error" || lvl === "panic") {
        emit("pause-game");
      }
    }
  },
);

// 分割条拖动 (Splitter)
function onSplitterPointerDown(e: PointerEvent) {
  if (!logContainerEl.value) return;
  isDraggingSplitter.value = true;
  (e.target as HTMLElement).setPointerCapture(e.pointerId);

  const startY = e.clientY;
  const startRatio = splitterRatio.value;
  const containerRect = logContainerEl.value.getBoundingClientRect();

  function onPointerMove(ev: PointerEvent) {
    if (!isDraggingSplitter.value) return;
    const dy = ev.clientY - startY;
    const newHeight = containerRect.height * startRatio + dy;
    const clampedRatio = Math.max(0.2, Math.min(0.85, newHeight / containerRect.height));
    splitterRatio.value = clampedRatio;
    syncLogViewport();
  }

  function onPointerUp(ev: PointerEvent) {
    isDraggingSplitter.value = false;
    (ev.target as HTMLElement).releasePointerCapture(ev.pointerId);
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerUp);
    syncLogViewport();
  }

  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);
}

let resizeObserver: ResizeObserver | null = null;

onMounted(() => {
  if (logScrollEl.value) {
    resizeObserver = new ResizeObserver(() => syncLogViewport());
    resizeObserver.observe(logScrollEl.value);
    syncLogViewport();
  }
});

onUnmounted(() => {
  if (resizeObserver) resizeObserver.disconnect();
});
</script>

<template>
  <div ref="logContainerEl" class="console-panel">
    <!-- Unity 风格控制台顶部工具条 -->
    <div class="console-toolbar">
      <button class="ghost icon tool-action-btn" :title="$t('console.clear')" @click="emit('clear')">
        <Trash2 :size="13" />
        <span>{{ $t("console.clear") }}</span>
      </button>

      <div class="console-divider" />

      <!-- Collapse / Clear on Play / Error Pause 切换 -->
      <div class="segmented-group">
        <button
          class="segmented-btn"
          :class="{ active: isCollapsed }"
          :title="$t('console.collapse')"
          @click="isCollapsed = !isCollapsed"
        >
          <Layers :size="12" />
          <span>Collapse</span>
        </button>
        <button
          class="segmented-btn"
          :class="{ active: clearOnPlay }"
          :title="$t('console.clearOnPlay')"
          @click="clearOnPlay = !clearOnPlay"
        >
          <span>Clear on Play</span>
        </button>
        <button
          class="segmented-btn"
          :class="{ active: errorPause }"
          :title="$t('console.errorPause')"
          @click="errorPause = !errorPause"
        >
          <span>Error Pause</span>
        </button>
      </div>

      <div class="console-divider" />

      <!-- 来源选择 -->
      <div class="log-filter">
        <button :class="{ active: logSourceFilter === 'all' }" @click="logSourceFilter = 'all'">
          {{ $t("console.all") }}
        </button>
        <button :class="{ active: logSourceFilter === 'editor' }" @click="logSourceFilter = 'editor'">
          {{ $t("console.editor") }}
        </button>
        <button :class="{ active: logSourceFilter === 'game' }" @click="logSourceFilter = 'game'">
          Game
        </button>
      </div>

      <!-- 搜索过滤框 -->
      <div class="console-search-box">
        <Search :size="12" class="search-icon" />
        <input
          v-model="searchQuery"
          type="text"
          class="search-input"
          :placeholder="$t('console.filterPlaceholder')"
        />
        <button v-if="searchQuery" class="ghost icon clear-search-btn" @click="searchQuery = ''">
          <X :size="11" />
        </button>
      </div>

      <!-- 日志级别下拉 -->
      <EditorDropdown
        :model-value="logLevelFilter"
        :options="logLevelOptions"
        :title="$t('console.title')"
        @update:model-value="(val) => (logLevelFilter = val as LogLevel | 'all')"
      />

      <span class="spacer" />

      <!-- Unity 风格三色药丸徽章 -->
      <div class="log-counts-group">
        <button
          class="log-count-btn info-count"
          :class="{ active: logLevelFilter === 'info' }"
          title="Info"
          @click="logLevelFilter = logLevelFilter === 'info' ? 'all' : 'info'"
        >
          <Info :size="12" />
          <span>{{ logCounts.info }}</span>
        </button>
        <button
          class="log-count-btn warn-count"
          :class="{ active: logLevelFilter === 'warn' }"
          title="Warning"
          @click="logLevelFilter = logLevelFilter === 'warn' ? 'all' : 'warn'"
        >
          <AlertTriangle :size="12" />
          <span>{{ logCounts.warn }}</span>
        </button>
        <button
          class="log-count-btn error-count"
          :class="{ active: logLevelFilter === 'error' }"
          title="Error"
          @click="logLevelFilter = logLevelFilter === 'error' ? 'all' : 'error'"
        >
          <AlertCircle :size="12" />
          <span>{{ logCounts.error }}</span>
        </button>
      </div>
    </div>

    <!-- 上半部分：日志列表 -->
    <div
      ref="logScrollEl"
      class="console-list-area"
      :style="{ flex: `0 0 ${splitterRatio * 100}%` }"
      @scroll.passive="onLogScroll"
    >
      <div class="log-virtual" :style="{ height: `${logWindow.total}px` }">
        <div
          class="log-window"
          :style="{ transform: `translateY(${logWindow.offset}px)` }"
        >
          <div
            v-for="(row, index) in visibleLogRows"
            :key="logWindow.start + index"
            class="log-row"
            :class="[
              `log-level-${row.level}`,
              { selected: selectedLog?.id === row.id },
            ]"
            @click="selectRow(row)"
          >
            <span class="log-row-icon">
              <AlertCircle v-if="row.level === 'error' || row.level === 'panic'" :size="13" />
              <AlertTriangle v-else-if="row.level === 'warn'" :size="13" />
              <Info v-else :size="13" />
            </span>
            <span class="log-row-text" :title="row.line">{{ row.line }}</span>
            <span v-if="isCollapsed && row.count && row.count > 1" class="log-collapse-badge">
              {{ row.count }}
            </span>
          </div>
        </div>
      </div>

      <div v-if="!displayLogs.length" class="empty">{{ $t("console.noLogs") }}</div>
    </div>

    <!-- 水平拖拽分割条 (Splitter) -->
    <div class="console-splitter" @pointerdown="onSplitterPointerDown" />

    <!-- 下半部分：选中日志详情堆栈区 (Log Detail View) -->
    <div class="console-detail-area">
      <div v-if="selectedLog" class="detail-container">
        <div class="detail-header">
          <span class="detail-badge" :class="`badge-${selectedLog.level}`">
            {{ selectedLog.level.toUpperCase() }}
          </span>
          <span class="detail-source-tag">[{{ selectedLog.source }}]</span>
          <span class="spacer" />
          <button class="ghost icon copy-detail-btn" :title="$t('console.copy')" @click="copySelected">
            <Check v-if="copied" :size="12" class="copy-success-icon" />
            <Copy v-else :size="12" />
            <span>{{ copied ? $t("console.copied") : $t("console.copy") }}</span>
          </button>
        </div>
        <div class="detail-body">
          <pre class="detail-text">{{ selectedLog.line }}</pre>
        </div>
      </div>
      <div v-else class="detail-empty">
        <span>{{ $t("console.noLogSelected") }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.console-panel {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  background: var(--bg-sunken, #1c1c1c);
  overflow: hidden;
  user-select: none;
}

.console-toolbar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  background: var(--chrome, #222222);
  border-bottom: 1px solid var(--border-strong, #333333);
  font-size: 11px;
}

.tool-action-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 8px;
  font-size: 11px;
  border-radius: 2px;
}

.console-divider {
  width: 1px;
  height: 14px;
  background: var(--border-strong, #383838);
  margin: 0 2px;
}

.console-search-box {
  position: relative;
  display: flex;
  align-items: center;
  width: 150px;
}

.search-icon {
  position: absolute;
  left: 6px;
  color: var(--text-muted, #777777);
  pointer-events: none;
}

.search-input {
  width: 100%;
  height: 22px;
  padding: 1px 20px 1px 22px;
  font-size: 11px;
  background: var(--bg-sunken, #161616);
  border: 1px solid var(--border-strong, #333333);
  border-radius: 2px;
  color: #ffffff;
}

.search-input:focus {
  border-color: var(--accent, #3a75c4);
  outline: none;
}

.clear-search-btn {
  position: absolute;
  right: 2px;
  width: 16px;
  height: 16px;
  color: var(--muted, #777777);
}

.console-list-area {
  min-height: 60px;
  overflow-y: auto;
  overflow-x: hidden;
  background: var(--bg-sunken, #1e1e1e);
}

.log-virtual {
  position: relative;
  min-height: 0;
}

.log-window {
  position: absolute;
  inset: 0 auto auto 0;
  width: 100%;
}

.log-row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 20px;
  line-height: 20px;
  padding: 0 8px;
  font-size: 11px;
  font-family: var(--font-mono, monospace);
  color: var(--text, #c4c4c4);
  cursor: pointer;
  white-space: nowrap;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
}

.log-row:hover {
  background: rgba(255, 255, 255, 0.05);
}

.log-row.selected {
  background: var(--accent, #3a75c4);
  color: #ffffff;
}

.log-row.selected .log-row-icon svg {
  color: #ffffff !important;
}

.log-row-icon {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  width: 14px;
}

.log-level-error .log-row-icon,
.log-level-panic .log-row-icon {
  color: var(--danger, #f44747);
}

.log-level-warn .log-row-icon {
  color: var(--warning, #e5a93c);
}

.log-level-info .log-row-icon,
.log-level-debug .log-row-icon,
.log-level-trace .log-row-icon {
  color: var(--accent-alt, #569cd6);
}

.log-row-text {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.log-collapse-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 14px;
  padding: 0 4px;
  font-size: 9.5px;
  font-weight: 600;
  border-radius: 7px;
  background: var(--panel-raised, #333333);
  color: var(--text-strong, #ffffff);
  border: 1px solid var(--border-strong, #444444);
  flex-shrink: 0;
}

.log-row.selected .log-collapse-badge {
  background: rgba(0, 0, 0, 0.35);
  border-color: rgba(255, 255, 255, 0.3);
}

.console-splitter {
  height: 4px;
  background: var(--border, #282828);
  border-top: 1px solid var(--border-strong, #333333);
  border-bottom: 1px solid var(--border-strong, #1a1a1a);
  cursor: ns-resize;
  flex-shrink: 0;
}

.console-splitter:hover {
  background: var(--accent, #3a75c4);
}

.console-detail-area {
  flex: 1 1 auto;
  min-height: 40px;
  background: var(--panel, #242424);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.detail-container {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.detail-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 8px;
  background: var(--chrome, #1f1f1f);
  border-bottom: 1px solid var(--border-strong, #333333);
  font-size: 11px;
}

.detail-badge {
  padding: 1px 5px;
  border-radius: 2px;
  font-size: 9.5px;
  font-weight: 600;
}

.badge-error,
.badge-panic {
  background: rgba(244, 71, 71, 0.2);
  color: var(--danger, #f44747);
  border: 1px solid rgba(244, 71, 71, 0.4);
}

.badge-warn {
  background: rgba(229, 169, 60, 0.2);
  color: var(--warning, #e5a93c);
  border: 1px solid rgba(229, 169, 60, 0.4);
}

.badge-info,
.badge-debug,
.badge-trace {
  background: rgba(86, 156, 214, 0.2);
  color: var(--accent-alt, #569cd6);
  border: 1px solid rgba(86, 156, 214, 0.4);
}

.detail-source-tag {
  color: var(--text-muted, #777777);
  font-size: 10px;
}

.copy-detail-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 20px;
  padding: 0 6px;
  font-size: 10.5px;
}

.copy-success-icon {
  color: var(--success, #4ec9b0);
}

.detail-body {
  flex: 1 1 auto;
  padding: 8px 10px;
  overflow-y: auto;
  user-select: text;
}

.detail-text {
  margin: 0;
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-strong, #eeeeee);
  white-space: pre-wrap;
  word-break: break-all;
}

.detail-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-muted, #666666);
  font-size: 11px;
}
</style>
