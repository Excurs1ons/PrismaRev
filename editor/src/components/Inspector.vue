<script setup lang="ts">
import { computed, ref } from "vue";
import FieldEditor from "./FieldEditor.vue";
import type { EntityJson, FieldSchema, JsonValue, Schema } from "../types";

/** 单实体检查器：name / parent / transform + 任意组件（schema 驱动）。 */
const props = defineProps<{
  entity: EntityJson;
  index: number;
  entities: EntityJson[];
  schema: Schema;
}>();

const emit = defineEmits<{
  (e: "update", entity: EntityJson): void;
  (e: "remove", index: number): void;
}>();

const addName = ref("");

function shortName(name: string): string {
  return name.split("::").pop() ?? name;
}

function patch(part: Partial<EntityJson>) {
  emit("update", { ...props.entity, ...part });
}

function setName(event: Event) {
  patch({ name: (event.target as HTMLInputElement).value });
}

function onParentChange(event: Event) {
  const raw = Number((event.target as HTMLSelectElement).value);
  patch({ parent: Number.isFinite(raw) && raw >= 0 ? raw : null });
}

function setTransform(key: "translation" | "rotation" | "scale", value: JsonValue) {
  patch({ transform: { ...props.entity.transform, [key]: value as number[] } });
}

function setComponent(name: string, value: JsonValue) {
  emit("update", {
    ...props.entity,
    components: { ...props.entity.components, [name]: value },
  });
}

function removeComponent(name: string) {
  const components = { ...props.entity.components };
  delete components[name];
  emit("update", { ...props.entity, components });
}

function addComponent() {
  const name = addName.value;
  if (!name) return;
  const type = props.schema.types.find((t) => t.name === name);
  setComponent(name, (type?.defaultJson as JsonValue) ?? {});
  addName.value = "";
}

function componentValue(name: string): JsonValue {
  return props.entity.components[name] as JsonValue;
}

/** 组件 JSON 值本身不是结构体时（`Name` 裸字符串等），仅作类型提示。 */
function componentField(name: string): FieldSchema | null {
  if (!props.schema.types.some((t) => t.name === name)) return null;
  return { name, kind: "struct", structType: name, inner: null, arrayLen: null, readOnly: false };
}

/** 合成一个定长 `f32[n]` 字段，用于 transform 三个向量。 */
function vecField(name: string, length: number): FieldSchema {
  return {
    name,
    kind: "array",
    structType: null,
    arrayLen: length,
    readOnly: false,
    inner: { name: "", kind: "f32", structType: null, inner: null, arrayLen: null, readOnly: false },
  };
}

const componentNames = computed(() => Object.keys(props.entity.components));

const availableComponents = computed(() =>
  props.schema.types
    .filter((t) => t.isComponent && !(t.name in props.entity.components))
    .map((t) => t.name),
);
</script>

<template>
  <div class="card">
    <header>
      <span class="title">实体 #{{ index }}</span>
      <span class="spacer" />
      <button
        class="ghost icon"
        :disabled="entities.length <= 1"
        title="删除实体（至少保留一个）"
        @click="emit('remove', index)"
      >
        删除
      </button>
    </header>
    <div class="card-body">
      <div class="row">
        <div class="label">name</div>
        <div>
          <input type="text" :value="entity.name ?? ''" @input="setName" />
        </div>
      </div>
      <div class="row">
        <div class="label">parent</div>
        <div>
          <select :value="entity.parent ?? -1" @change="onParentChange">
            <option value="-1">（无父）</option>
            <option v-for="(e, i) in entities" :key="i" :value="i" :disabled="i === index">
              {{ e.name || `#${i}` }} · #{{ i }}
            </option>
          </select>
        </div>
      </div>
    </div>
    <div class="card-body" style="border-top: 1px solid var(--border)">
      <div class="hint" style="margin-bottom: 4px">transform（局部 · T×R×S）</div>
      <FieldEditor
        label="translation"
        :value="entity.transform.translation"
        :field="vecField('translation', 3)"
        :schema="schema"
        @update="(v) => setTransform('translation', v)"
      />
      <FieldEditor
        label="rotation"
        :value="entity.transform.rotation"
        :field="vecField('rotation', 4)"
        :schema="schema"
        @update="(v) => setTransform('rotation', v)"
      />
      <FieldEditor
        label="scale"
        :value="entity.transform.scale"
        :field="vecField('scale', 3)"
        :schema="schema"
        @update="(v) => setTransform('scale', v)"
      />
    </div>
  </div>

  <div v-for="name in componentNames" :key="name" class="card">
    <header>
      <span class="title">{{ shortName(name) }}</span>
      <span class="hint">{{ name }}</span>
      <span class="spacer" />
      <button class="ghost icon" title="移除组件" @click="removeComponent(name)">移除</button>
    </header>
    <div class="card-body">
      <FieldEditor
        :value="componentValue(name)"
        :field="componentField(name)"
        :schema="schema"
        @update="(v) => setComponent(name, v)"
      />
    </div>
  </div>

  <div class="card">
    <header><span class="title">添加组件</span></header>
    <div class="card-body">
      <div style="display: flex; gap: 8px">
        <select v-model="addName">
          <option value="">选择类型…</option>
          <option v-for="n in availableComponents" :key="n" :value="n">{{ shortName(n) }}</option>
        </select>
        <button class="primary" :disabled="!addName" @click="addComponent">添加</button>
      </div>
    </div>
  </div>
</template>
