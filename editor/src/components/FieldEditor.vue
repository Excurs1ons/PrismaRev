<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { Minus, Plus } from "lucide-vue-next";
import ColorField from "./ColorField.vue";
import NumberField from "./NumberField.vue";
import { ASSETS_KEY, ASSET_PICKER_KEY } from "../keys";
import { isColorField } from "../format";
import type { FieldSchema, JsonValue, Schema } from "../types";

/**
 * 递归字段编辑器：按 **JSON 值的运行时形状** 选控件，schema（`field`）
 * 只作为类型/只读/嵌套结构体的提示。这样 `Name`（裸字符串）、
 * `Active`（裸布尔）这类「JSON 形状 ≠ 派生结构体」的组件也能正确编辑。
 *
 * `path` 是字段的稳定编辑路径（组件内相对路径，如 `anchors.min_x`），随每次
 * 改动一并向上抛：宿主用它做「同字段连续输入合并成一次撤销」，
 * 也用它调用 Rust 侧的**反射字段写入**。
 */
const props = defineProps<{
  label?: string;
  path?: string;
  value: JsonValue;
  field?: FieldSchema | null;
  schema: Schema;
}>();

const emit = defineEmits<{ (e: "update", value: JsonValue, path: string): void }>();

/** 离散操作（增删）用唯一后缀，避免与连续输入合并。 */
let opSeq = 0;

type Kind = "bool" | "number" | "string" | "object" | "array" | "null";

function kindOf(v: JsonValue | undefined): Kind {
  if (v === null || v === undefined) return "null";
  if (typeof v === "boolean") return "bool";
  if (typeof v === "number") return "number";
  if (typeof v === "string") return "string";
  if (Array.isArray(v)) return "array";
  return "object";
}

const hint = computed<FieldSchema | null>(() => props.field ?? null);
const kind = computed(() => kindOf(props.value));
/** 定长数组（`[T; N]`）：长度由 schema 固定，不能增删元素。 */
const isFixedArray = computed(() => hint.value?.kind === "array");
const pathStr = computed(() => props.path ?? "");

const isInteger = computed(() =>
  ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].includes(hint.value?.kind ?? ""),
);

const isColor = computed(() => isColorField(hint.value?.name, props.value));

// —— 资产引用：对「看起来是资源路径」的字符串字段给出候选与选择器 ——
const assets = inject(ASSETS_KEY, null);
const openAssetPicker = inject(ASSET_PICKER_KEY, null);

const ASSET_EXTENSIONS = [
  ".gltf", ".glb", ".obj", ".fbx", ".bin",
  ".hdr", ".exr", ".png", ".jpg", ".ktx2",
  ".rscn", ".pak", ".scene.json",
];
const ASSET_FIELD_PATTERN = /(^|_)(path|file|map|volume|hdr)$/;

const showAssetPicker = computed(() => {
  if (kind.value !== "string") return false;
  const name = hint.value?.name ?? "";
  if (ASSET_FIELD_PATTERN.test(name)) return true;
  const value = (props.value as string).toLowerCase();
  return value.length > 0 && ASSET_EXTENSIONS.some((ext) => value.endsWith(ext));
});

const assetOptions = computed(() =>
  (assets?.value ?? []).map((asset) => asset.path).slice(0, 1000),
);

/** datalist 需要唯一 id；字段路径天然唯一。 */
const listId = computed(() => `asset-${pathStr.value.replace(/[^a-zA-Z0-9_-]/g, "_")}`);

const asArray = computed<JsonValue[]>(() => (Array.isArray(props.value) ? props.value : []));
const asObject = computed<Record<string, JsonValue>>(() =>
  props.value && typeof props.value === "object" && !Array.isArray(props.value)
    ? (props.value as Record<string, JsonValue>)
    : {},
);

/** 嵌套结构体字段表（由 `field.structType` 指向的类型给出）。 */
const structFields = computed<FieldSchema[]>(() => {
  const name = hint.value?.structType;
  if (!name) return [];
  return props.schema.types.find((t) => t.name === name)?.fields ?? [];
});

const objectEntries = computed(() =>
  Object.keys(asObject.value).map((key) => ({ key, value: asObject.value[key] })),
);

/** schema 里存在、但当前 JSON 未写的字段（通常是可选字段）。 */
const missingKeys = computed(() =>
  structFields.value.filter((f) => !(f.name in asObject.value)),
);

function fieldFor(key: string): FieldSchema | null {
  return structFields.value.find((f) => f.name === key) ?? null;
}

function childPath(key: string): string {
  return pathStr.value ? `${pathStr.value}.${key}` : key;
}

function itemPath(index: number): string {
  return `${pathStr.value}[${index}]`;
}

/** 某字段的「零值」默认，用于「添加」操作。 */
function defaultFor(field: FieldSchema | null): JsonValue {
  if (!field) return null;
  switch (field.kind) {
    case "bool":
      return false;
    case "string":
      return "";
    case "option":
      return null;
    case "list":
      return [];
    case "array":
      return Array.from({ length: field.arrayLen ?? 0 }, () => defaultFor(field.inner ?? null));
    case "struct": {
      const target = field.structType
        ? props.schema.types.find((t) => t.name === field.structType)
        : undefined;
      const obj: Record<string, JsonValue> = {};
      for (const f of target?.fields ?? []) obj[f.name] = defaultFor(f);
      return obj;
    }
    default:
      return 0;
  }
}

/** `fromChild` 为嵌套编辑器冒泡上来的精确路径；为空则用本字段路径。 */
function set(next: JsonValue, fromChild?: string) {
  emit("update", next, fromChild ?? pathStr.value);
}

/** 离散操作：唯一后缀确保不被合并。 */
function setOp(next: JsonValue) {
  opSeq += 1;
  emit("update", next, `${pathStr.value}#op${opSeq}`);
}

function setKey(key: string, value: JsonValue, childPathValue: string) {
  set({ ...asObject.value, [key]: value }, childPathValue);
}

function setItem(index: number, value: JsonValue, childPathValue: string) {
  const next = [...asArray.value];
  next[index] = value;
  set(next, childPathValue);
}

function addItem() {
  // 定长数组拒绝增删：长度不匹配会被 `Reflect::set_field` 拒绝。
  if (isFixedArray.value) return;
  setOp([...asArray.value, defaultFor(hint.value?.inner ?? null)]);
}

function removeItem(index: number) {
  if (isFixedArray.value) return;
  const next = [...asArray.value];
  next.splice(index, 1);
  setOp(next);
}

/** 把 `null` 折叠成一个有意义的值（option → 内层默认）。 */
function initFromNull() {
  const h = hint.value;
  if (h?.kind === "option" && h.inner) {
    setOp(defaultFor(h.inner));
    return;
  }
  setOp(defaultFor(h));
}

async function pickAsset() {
  if (!openAssetPicker) return;
  const picked = await openAssetPicker((props.value as string) ?? "");
  if (picked !== null && picked !== undefined) setOp(picked);
}

// —— 从资产管理器拖拽到字段 ——
const dragOver = ref(false);

function isAssetDrag(event: DragEvent): boolean {
  return Array.from(event.dataTransfer?.types ?? []).includes("application/x-prism-asset");
}

function onAssetDragOver(event: DragEvent) {
  if (kind.value !== "string" || hint.value?.readOnly || !isAssetDrag(event)) return;
  event.preventDefault();
  dragOver.value = true;
}

function onAssetDrop(event: DragEvent) {
  dragOver.value = false;
  if (kind.value !== "string" || hint.value?.readOnly) return;
  const path = event.dataTransfer?.getData("application/x-prism-asset");
  if (!path) return;
  event.preventDefault();
  setOp(path);
}

function onText(event: Event) {
  set((event.target as HTMLInputElement).value);
}
function onBool(event: Event) {
  set((event.target as HTMLInputElement).checked);
}
</script>

<template>
  <!-- 对象：按 JSON 的键渲染，schema 提供字段类型/只读/嵌套类型 -->
  <div v-if="kind === 'object'">
    <div class="row" :class="{ 'no-label': !label }" :data-field="pathStr">
      <div v-if="label" class="label">
        {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
      </div>
      <div class="hint">
        {{ structFields.length ? "结构体" : "对象" }} · {{ objectEntries.length }} 字段
      </div>
    </div>
    <div class="nested">
      <FieldEditor
        v-for="entry in objectEntries"
        :key="entry.key"
        :label="entry.key"
        :path="childPath(entry.key)"
        :value="entry.value"
        :field="fieldFor(entry.key)"
        :schema="schema"
        @update="(v, p) => setKey(entry.key, v, p)"
      />
      <div v-for="f in missingKeys" :key="`missing-${f.name}`" class="row">
        <div class="label">{{ f.name }}</div>
        <div>
          <button
            class="ghost icon"
            :disabled="f.readOnly"
            @click="setOp({ ...asObject, [f.name]: defaultFor(f) })"
          >
            + 添加
          </button>
        </div>
      </div>
      <div v-if="!objectEntries.length && !missingKeys.length" class="hint">（空）</div>
    </div>
  </div>

  <!-- 颜色：取色盘 + 通道数值 + alpha -->
  <div v-else-if="isColor">
    <div class="row" :class="{ 'no-label': !label }" :data-field="pathStr">
      <div v-if="label" class="label">
        {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
      </div>
      <ColorField
        :model-value="(value as number[])"
        :disabled="hint?.readOnly"
        @update:model-value="(v) => set(v)"
      />
    </div>
  </div>

  <!-- 数组 / 列表 -->
  <div v-else-if="kind === 'array'">
    <div class="row" :class="{ 'no-label': !label }" :data-field="pathStr">
      <div v-if="label" class="label">
        {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
      </div>
      <div class="hint">
        {{ isFixedArray ? `定长 ${hint?.arrayLen ?? "?"} · ${asArray.length} 项` : `列表 · ${asArray.length} 项` }}
      </div>
    </div>
    <div class="nested">
      <div v-for="(item, i) in asArray" :key="i" class="array-row">
        <span class="idx">[{{ i }}]</span>
        <FieldEditor
          :path="itemPath(i)"
          :value="item"
          :field="hint?.inner ?? null"
          :schema="schema"
          @update="(v, p) => setItem(i, v, p)"
        />
        <button
          v-if="!isFixedArray"
          class="ghost icon"
          :disabled="hint?.readOnly"
          title="删除"
          @click="removeItem(i)"
        >
          <Minus :size="13" />
        </button>
      </div>
      <button v-if="!isFixedArray" class="ghost" :disabled="hint?.readOnly" @click="addItem">
        <Plus :size="13" /> 元素
      </button>
    </div>
  </div>

  <!-- 标量 -->
  <div v-else class="row" :class="{ 'no-label': !label }" :data-field="pathStr">
    <div v-if="label" class="label">
      {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
    </div>
    <div
      class="scalar"
      :class="{ 'drop-target': dragOver }"
      @dragover="onAssetDragOver"
      @dragleave="dragOver = false"
      @drop="onAssetDrop"
    >
      <input
        v-if="kind === 'bool'"
        type="checkbox"
        :checked="value === true"
        :disabled="hint?.readOnly"
        @change="onBool"
      />
      <NumberField
        v-else-if="kind === 'number'"
        :model-value="(value as number)"
        :integer="isInteger"
        :disabled="hint?.readOnly"
        @update:model-value="(v) => set(v)"
      />
      <template v-else-if="kind === 'string'">
        <input
          type="text"
          :list="showAssetPicker ? listId : undefined"
          :disabled="hint?.readOnly"
          :value="(value as string)"
          @input="onText"
        />
        <button
          v-if="showAssetPicker"
          class="ghost icon"
          :disabled="hint?.readOnly"
          title="资产管理器"
          @click="pickAsset"
        >
          …
        </button>
        <datalist v-if="showAssetPicker" :id="listId">
          <option v-for="asset in assetOptions" :key="asset" :value="asset" />
        </datalist>
      </template>
      <template v-else>
        <span class="hint">null</span>
        <button class="ghost icon" :disabled="hint?.readOnly" @click="initFromNull">设置</button>
      </template>
    </div>
  </div>
</template>
