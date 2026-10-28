<script setup lang="ts">
import { onUnmounted, ref, watch } from "vue";
import { formatNumber } from "../format";

/**
 * 数值输入框：支持**横向拖动调节**、方向键微调、直接输入。
 *
 * - 拖动：在框内按下并左右移动（3px 起判）；`Shift` 精调 0.1×，`Alt` 粗调 10×。
 * - 方向键：`↑/↓` 按 step；同样受 `Shift`/`Alt` 影响。
 * - 输入：回车/失焦提交，`Esc` 还原。
 */
const props = defineProps<{
  modelValue: number;
  step?: number;
  min?: number;
  max?: number;
  integer?: boolean;
  disabled?: boolean;
}>();

const emit = defineEmits<{ (e: "update:modelValue", value: number): void }>();

const input = ref<HTMLInputElement | null>(null);
const text = ref(formatNumber(props.modelValue));
const editing = ref(false);
const dragging = ref(false);

let startX = 0;
let startValue = 0;

watch(
  () => props.modelValue,
  (value) => {
    if (!editing.value) text.value = formatNumber(value);
  },
);

function stepSize(): number {
  return props.step ?? (props.integer ? 1 : 0.01);
}

function round(value: number): number {
  return props.integer ? Math.round(value) : Math.round(value * 1e6) / 1e6;
}

function commit(value: number) {
  let next = round(value);
  if (props.min !== undefined && next < props.min) {
    next = props.min;
  }
  if (props.max !== undefined && next > props.max) {
    next = props.max;
  }
  text.value = formatNumber(next);
  if (next !== props.modelValue) emit("update:modelValue", next);
}

function modifierScale(event: { shiftKey: boolean; altKey: boolean }): number {
  return event.shiftKey ? 10 : event.altKey ? 0.1 : 1;
}

function onFocus(event: FocusEvent) {
  editing.value = true;
  (event.target as HTMLInputElement).select();
}

function onBlur() {
  editing.value = false;
  const parsed = Number(text.value);
  if (Number.isFinite(parsed)) {
    commit(parsed);
  } else {
    text.value = formatNumber(props.modelValue);
  }
}

function onKeydown(event: KeyboardEvent) {
  const target = event.target as HTMLInputElement;

  if (event.key === "Enter") {
    const parsed = Number(target.value);
    if (Number.isFinite(parsed)) commit(parsed);
    target.blur();
    return;
  }
  if (event.key === "Escape") {
    text.value = formatNumber(props.modelValue);
    editing.value = false;
    target.blur();
    return;
  }
  if (event.key === "ArrowUp" || event.key === "ArrowDown") {
    event.preventDefault();
    const parsed = Number(target.value);
    const base = Number.isFinite(parsed) ? parsed : props.modelValue;
    const direction = event.key === "ArrowUp" ? 1 : -1;
    commit(base + direction * stepSize() * modifierScale(event));
  }
}

function onPointerDown(event: PointerEvent) {
  if (props.disabled || event.button !== 0) return;
  startX = event.clientX;
  startValue = props.modelValue;
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", finishDrag, { once: true });
  window.addEventListener("pointercancel", finishDrag, { once: true });
}

function onPointerMove(event: PointerEvent) {
  const dx = event.clientX - startX;
  if (!dragging.value) {
    if (Math.abs(dx) < 3) return;
    dragging.value = true;
    editing.value = false;
    input.value?.blur();
  }
  event.preventDefault();
  commit(startValue + dx * stepSize() * modifierScale(event));
}

function finishDrag() {
  window.removeEventListener("pointermove", onPointerMove);
  window.removeEventListener("pointerup", finishDrag);
  window.removeEventListener("pointercancel", finishDrag);
  dragging.value = false;
}

onUnmounted(finishDrag);
</script>

<template>
  <div class="num" :class="{ dragging }">
    <input
      ref="input"
      type="text"
      inputmode="decimal"
      :value="text"
      :disabled="disabled"
      title="左右拖动调节；方向键微调（Shift 精调 / Alt 粗调）"
      @focus="onFocus"
      @blur="onBlur"
      @input="(event) => (text = (event.target as HTMLInputElement).value)"
      @keydown="onKeydown"
      @pointerdown="onPointerDown"
    />
  </div>
</template>
