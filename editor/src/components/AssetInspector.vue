<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import {
  Box,
  Copy,
  ExternalLink,
  FileCode,
  FileText,
  Image as ImageIcon,
  Music,
} from "lucide-vue-next";
import * as api from "../api";
import { formatBytes } from "../format";
import type { AssetInfo, AssetRef } from "../types";

/**
 * 资产检查器（Asset Inspector 规范）：
 * - 当在 Project / 资产管理器中选中资产时，在 Inspector 面板展示该资产的导入配置、引用关系与大图/文本预览。
 * - 底部提供 规范的 Asset Preview 区域。
 */
const props = defineProps<{
  asset: AssetInfo;
}>();

const emit = defineEmits<{
  (e: "jump", ref: AssetRef): void;
  (e: "open", path: string): void;
}>();

type PreviewKind = "image" | "text" | "document" | "none";

const IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
  avif: "image/avif",
};

const TEXT_EXT = new Set([
  "txt", "md", "json", "toml", "ron", "yaml", "yml", "xml", "csv", "ini", "cfg", "log",
  "slang", "wgsl", "glsl", "vert", "frag", "comp", "hlsl", "shader",
]);

function previewKindOf(path: string): PreviewKind {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (path.endsWith(".scene.json") || path.endsWith(".prefab.json")) return "document";
  if (ext in IMAGE_MIME) return "image";
  if (TEXT_EXT.has(ext)) return "text";
  return "none";
}

const previewKind = computed(() => previewKindOf(props.asset.path));
const previewUrl = ref("");
const previewText = ref("");
const previewTruncated = ref(false);
const previewError = ref("");
const previewBusy = ref(false);
const imageDimensions = ref<{ width: number; height: number } | null>(null);

let previewObjectUrl: string | null = null;
let previewToken = 0;

const usage = ref<AssetRef[]>([]);
const usageBusy = ref(false);
const usageError = ref("");
let usageToken = 0;

const assetGuid = ref("");
const copiedPath = ref(false);
const copiedGuid = ref(false);

function clearPreview() {
  previewToken += 1;
  if (previewObjectUrl) {
    URL.revokeObjectURL(previewObjectUrl);
    previewObjectUrl = null;
  }
  previewUrl.value = "";
  previewText.value = "";
  previewTruncated.value = false;
  previewError.value = "";
  previewBusy.value = false;
  imageDimensions.value = null;
}

async function loadPreview(asset: AssetInfo) {
  const token = ++previewToken;
  clearPreview();
  previewBusy.value = true;
  try {
    const kind = previewKindOf(asset.path);
    if (kind === "image") {
      const bytes = await api.readAssetBytes(asset.path);
      if (token !== previewToken) return;
      const ext = asset.path.split(".").pop()?.toLowerCase() ?? "png";
      const mime = IMAGE_MIME[ext] ?? "application/octet-stream";
      const blob = new Blob([bytes], { type: mime });
      previewObjectUrl = URL.createObjectURL(blob);
      previewUrl.value = previewObjectUrl;

      // 读取图片真实尺寸
      const img = new Image();
      img.onload = () => {
        if (token === previewToken) {
          imageDimensions.value = { width: img.naturalWidth, height: img.naturalHeight };
        }
      };
      img.src = previewObjectUrl;
    } else if (kind === "text") {
      const result = await api.readAssetText(asset.path);
      if (token !== previewToken) return;
      previewText.value = result.text;
      previewTruncated.value = result.truncated;
    }
  } catch (e) {
    if (token === previewToken) previewError.value = `${e}`;
  } finally {
    if (token === previewToken) previewBusy.value = false;
  }
}

async function loadUsage(asset: AssetInfo) {
  const token = ++usageToken;
  usage.value = [];
  usageError.value = "";
  usageBusy.value = true;
  try {
    const refs = await api.assetUsage(asset.path);
    if (token !== usageToken) return;
    usage.value = refs;
  } catch (e) {
    if (token !== usageToken) return;
    usageError.value = `${e}`;
  } finally {
    if (token === usageToken) usageBusy.value = false;
  }
}

async function loadGuid(asset: AssetInfo) {
  try {
    assetGuid.value = await api.assetGuid(asset.path);
  } catch {
    assetGuid.value = "—";
  }
}

watch(
  () => props.asset.path,
  () => {
    void loadPreview(props.asset);
    void loadUsage(props.asset);
    void loadGuid(props.asset);
  },
  { immediate: true },
);

onUnmounted(() => {
  if (previewObjectUrl) {
    URL.revokeObjectURL(previewObjectUrl);
    previewObjectUrl = null;
  }
});

const fileName = computed(() => props.asset.path.split("/").pop() ?? props.asset.path);

const fileExtension = computed(() => props.asset.path.split(".").pop()?.toUpperCase() ?? "FILE");

async function copyRelativePath() {
  await navigator.clipboard.writeText(props.asset.path);
  copiedPath.value = true;
  setTimeout(() => (copiedPath.value = false), 1500);
}

async function copyGuid() {
  if (!assetGuid.value) return;
  await navigator.clipboard.writeText(assetGuid.value);
  copiedGuid.value = true;
  setTimeout(() => (copiedGuid.value = false), 1500);
}
</script>

<template>
  <div class="asset-inspector">
    <!--资产标头 (Asset Header) -->
    <div class="asset-inspector-header">
      <div class="asset-icon-box">
        <ImageIcon v-if="previewKind === 'image'" :size="24" class="icon-image" />
        <FileCode v-else-if="props.asset.path.endsWith('.json') || props.asset.path.endsWith('.toml')" :size="24" class="icon-code" />
        <Box v-else-if="props.asset.path.endsWith('.glb') || props.asset.path.endsWith('.gltf') || props.asset.path.endsWith('.rscn')" :size="24" class="icon-model" />
        <Music v-else-if="props.asset.path.endsWith('.wav') || props.asset.path.endsWith('.ogg')" :size="24" class="icon-audio" />
        <FileText v-else :size="24" class="icon-default" />
      </div>
      <div class="asset-meta-main">
        <div class="asset-name-title" :title="fileName">{{ fileName }}</div>
        <div class="asset-sub-meta">
          <span class="badge">{{ fileExtension }}</span>
          <span>{{ formatBytes(asset.sizeBytes) }}</span>
        </div>
      </div>
    </div>

    <!-- 导入设置与元数据 (Import Settings & Metadata) -->
    <div class="card">
      <header>
        <span class="title">Import Settings</span>
        <span class="spacer" />
        <span class="hint">{{ asset.importer }}</span>
      </header>
      <div class="card-body">
        <div class="row">
          <div class="label">Kind</div>
          <div class="text-val">{{ asset.kind }}</div>
        </div>
        <div class="row">
          <div class="label">Importer</div>
          <div class="text-val">{{ asset.importer }}</div>
        </div>
        <div class="row">
          <div class="label">Path</div>
          <div class="scalar">
            <input type="text" readonly :value="asset.path" :title="asset.path" />
            <button class="ghost icon" :title="copiedPath ? '已复制' : '复制路径'" @click="copyRelativePath">
              <Copy :size="12" />
            </button>
          </div>
        </div>
        <div class="row">
          <div class="label">GUID</div>
          <div class="scalar">
            <input type="text" readonly :value="assetGuid" :title="assetGuid" />
            <button class="ghost icon" :title="copiedGuid ? '已复制' : '复制 GUID'" @click="copyGuid">
              <Copy :size="12" />
            </button>
          </div>
        </div>
        <div v-if="previewKind === 'document'" style="margin-top: 6px; display: flex; justify-content: flex-end">
          <button class="primary" @click="emit('open', asset.path)">
            <ExternalLink :size="12" /> 打开文档
          </button>
        </div>
      </div>
    </div>

    <!-- 引用关系卡片 (References / Usages) -->
    <div class="card">
      <header>
        <span class="title">References</span>
        <span class="spacer" />
        <span v-if="usageBusy" class="hint">查询中…</span>
        <span v-else-if="usage.length" class="hint">{{ usage.length }} 处引用</span>
        <span v-else class="hint">未被引用</span>
      </header>
      <div class="card-body">
        <div v-if="usageError" class="error">{{ usageError }}</div>
        <div v-else-if="usage.length" class="usage-list">
          <button
            v-for="(ref, i) in usage"
            :key="i"
            class="usage-link-btn"
            :title="ref.kind === 'manifest' ? '构建清单 (assets/scenes.json)' : '打开文档并高亮实体字段'"
            @click="emit('jump', ref)"
          >
            <template v-if="ref.kind === 'manifest'">
              <strong>[清单]</strong> {{ ref.scene }} · {{ ref.entity }}
            </template>
            <template v-else>
              <strong>[{{ ref.scene }}]</strong> #{{ ref.entityIndex }} {{ ref.entity }} · {{ ref.component.split("::").pop() }}.{{ ref.field }}
            </template>
          </button>
        </div>
        <div v-else class="hint" style="padding: 4px 0">
          未被当前场景、预制体或构建清单直接引用。
        </div>
      </div>
    </div>

    <!--底部资产预览区域 (Asset Preview) -->
    <div class="card preview-card">
      <header>
        <span class="title">Preview</span>
        <span class="spacer" />
        <span v-if="imageDimensions" class="hint">
          {{ imageDimensions.width }} × {{ imageDimensions.height }} px
        </span>
        <span v-else-if="previewKind === 'text'" class="hint">
          {{ previewText.split('\n').length }} lines
        </span>
      </header>
      <div class="card-body preview-card-body">
        <div v-if="previewBusy" class="hint preview-loading">加载预览中…</div>
        <div v-else-if="previewError" class="error">{{ previewError }}</div>

        <!-- 图片预览 (含透明度棋盘格背景与尺寸展示) -->
        <div v-else-if="previewKind === 'image' && previewUrl" class="image-preview-container">
          <img :src="previewUrl" :alt="asset.path" class="inspector-preview-image" />
          <div v-if="imageDimensions" class="preview-dimension-badge">
            {{ imageDimensions.width }} × {{ imageDimensions.height }}
          </div>
        </div>

        <!-- 文本 / 代码预览 -->
        <div v-else-if="previewKind === 'text'" class="text-preview-container">
          <pre class="inspector-preview-text">{{ previewText }}</pre>
          <div v-if="previewTruncated" class="asset-preview-note">已截断，仅显示前 512 KB</div>
        </div>

        <!-- 其它格式的说明占位 -->
        <div v-else class="empty" style="padding: 16px 0">
          该文件格式（{{ fileExtension }}）暂无图形预览。
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.asset-inspector {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.asset-inspector-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  background: var(--panel);
  border-bottom: 1px solid var(--border-strong);
}

.asset-icon-box {
  width: 36px;
  height: 36px;
  border-radius: 4px;
  background: var(--panel-raised);
  border: 1px solid var(--border-strong);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.icon-image { color: #4ec9b0; }
.icon-code { color: #569cd6; }
.icon-model { color: #dcdcaa; }
.icon-audio { color: #c586c0; }
.icon-default { color: #888888; }

.asset-meta-main {
  flex: 1 1 auto;
  min-width: 0;
}

.asset-name-title {
  font-weight: 600;
  font-size: 13px;
  color: var(--text-strong);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.asset-sub-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 10px;
  color: var(--muted);
  margin-top: 2px;
}

.asset-sub-meta .badge {
  background: var(--panel-raised);
  border: 1px solid var(--border-strong);
  padding: 0 4px;
  border-radius: 2px;
  font-weight: 600;
  color: var(--text-soft);
}

.text-val {
  font-size: 11px;
  color: var(--text-strong);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.usage-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.usage-link-btn {
  display: block;
  width: 100%;
  text-align: left;
  padding: 3px 6px;
  font-size: 11px;
  color: var(--accent-alt);
  background: var(--panel-alt);
  border: 1px solid var(--border-subtle);
  border-radius: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.usage-link-btn:hover {
  background: var(--row-hover);
  color: #ffffff;
  border-color: var(--accent);
}

.usage-link-btn strong {
  color: var(--text-soft);
}

.preview-card {
  margin-top: 4px;
}

.preview-card-body {
  padding: 6px;
  display: flex;
  flex-direction: column;
  align-items: center;
}

.image-preview-container {
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  position: relative;
  min-height: 140px;
  background:
    repeating-conic-gradient(var(--panel-alt) 0% 25%, var(--bg-sunken) 0% 50%) 50% / 16px 16px;
  border: 1px solid var(--border);
  border-radius: 3px;
  overflow: hidden;
  padding: 8px;
}

.inspector-preview-image {
  max-width: 100%;
  max-height: 240px;
  object-fit: contain;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.4);
}

.preview-dimension-badge {
  position: absolute;
  bottom: 4px;
  right: 4px;
  font-size: 10px;
  color: #fff;
  background: rgba(0, 0, 0, 0.7);
  padding: 1px 4px;
  border-radius: 2px;
  pointer-events: none;
}

.text-preview-container {
  width: 100%;
}

.inspector-preview-text {
  max-height: 240px;
  margin: 0;
  padding: 6px;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: 3px;
  background: var(--bg-sunken);
  color: var(--text-soft);
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  font-size: 10.5px;
  line-height: 1.4;
  white-space: pre;
}

.preview-loading {
  padding: 24px 0;
  text-align: center;
}
</style>
