<script setup lang="ts">
import { computed, inject } from "vue";
import { ASSETS_KEY } from "../keys";
import type { FieldSchema, JsonValue, Schema } from "../types";

/**
 * 递归字段编辑器：按 **JSON 值的运行时形状** 选控件，schema（`field`）
 * 只作为类型/只读/嵌套结构体的提示。这样 `Name`（裸字符串）、
 * `Active`（裸布尔）这类「JSON 形状 ≠ 派生结构体」的组件也能正确编辑。
 *
 * `path` 是字段的稳定编辑路径（如 `components.X.font_size`），随每次改动
 * 一并向上抛，供宿主做「同一字段的连续输入合并成一次撤销」。
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
const pathStr = computed(() => props.path ?? props.label ?? "");

// —— 资产引用：对「看起来是资源路径」的字符串字段给出候选列表 ——
const assets = inject(ASSETS_KEY, null);
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

const assetOptions = computed(() => (assets?.value ?? []).slice(0, 1000));

/** datalist 需要唯一 id；字段路径天然唯一。 */
const listId = computed(() => `asset-${pathStr.value.replace(/[^a-zA-Z0-9_-]/g, "_")}`);

const isInteger = computed(() =>
  ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].includes(hint.value?.kind ?? ""),
);

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
  setOp([...asArray.value, defaultFor(hint.value?.inner ?? null)]);
}

function removeItem(index: number) {
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

function onNumber(event: Event) {
  set(Number((event.target as HTMLInputElement).value));
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
    <div class="row" :class="{ 'no-label': !label }">
      <div v-if="label" class="label">
        {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
      </div>
      <div class="hint">
        {{ structFields.length ? '结构体' : '对象' }} · {{ objectEntries.length }} 字段
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

  <!-- 数组 / 列表 -->
  <div v-else-if="kind === 'array'">
    <div class="row" :class="{ 'no-label': !label }">
      <div v-if="label" class="label">
        {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
      </div>
      <div class="hint">
        {{ hint?.kind === 'array' ? `定长 ${hint?.arrayLen ?? '?'}` : '列表' }} ·
        {{ asArray.length }} 项
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
        <button class="ghost icon" :disabled="hint?.readOnly" title="删除" @click="removeItem(i)">
          −
        </button>
      </div>
      <button class="ghost" :disabled="hint?.readOnly" @click="addItem">+ 元素</button>
    </div>
  </div>

  <!-- 标量 -->
  <div v-else class="row" :class="{ 'no-label': !label }">
    <div v-if="label" class="label">
      {{ label }}<span v-if="hint?.readOnly" class="ro"> ·只读</span>
    </div>
    <div>
      <input
        v-if="kind === 'bool'"
        type="checkbox"
        :checked="value === true"
        :disabled="hint?.readOnly"
        @change="onBool"
      />
      <input
        v-else-if="kind === 'number'"
        type="number"
        :step="isInteger ? 1 : 'any'"
        :disabled="hint?.readOnly"
        :value="(value as number)"
        @input="onNumber"
      />
      <template v-else-if="kind === 'string'">
        <input
          type="text"
          :list="showAssetPicker ? listId : undefined"
          :disabled="hint?.readOnly"
          :value="(value as string)"
          @input="onText"
        />
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
