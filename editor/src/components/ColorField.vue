<script setup lang="ts">
import { computed } from "vue";
import NumberField from "./NumberField.vue";
import { clamp01, floatsToHex, hexToFloats } from "../format";
import type { JsonValue } from "../types";

/**
 * 颜色字段：取色盘 + 逐通道数值 + alpha 滑块。
 *
 * 引擎里的颜色都是归一化 `[r,g,b]` / `[r,g,b,a]`；取色盘按 8bit sRGB 近似映射
 * （不引入色彩管理），数值通道才是精确编辑。
 */
const props = withDefaults(
  defineProps<{ modelValue: number[]; disabled?: boolean; showAlpha?: boolean }>(),
  { showAlpha: true },
);
const emit = defineEmits<{ (e: "update:modelValue", value: JsonValue): void }>();

const hex = computed(() => floatsToHex(props.modelValue));
const hasAlpha = computed(() => props.showAlpha && props.modelValue.length === 4);
/** 隐藏 alpha 时连第 4 个通道的数值框一起隐藏（Unity `[ColorUsage(showAlpha: false)]`）。 */
const channels = computed(() =>
  props.showAlpha ? props.modelValue : props.modelValue.slice(0, 3),
);

function setChannel(index: number, value: number) {
  const next = props.modelValue.slice();
  next[index] = index === 3 ? clamp01(value) : value;
  emit("update:modelValue", next);
}

function setHex(value: string) {
  const [r, g, b] = hexToFloats(value);
  const next = props.modelValue.slice();
  next[0] = r;
  next[1] = g;
  next[2] = b;
  emit("update:modelValue", next);
}
</script>

<template>
  <div class="color">
    <input
      class="color-swatch"
      type="color"
      :value="hex"
      :disabled="disabled"
      title="取色盘"
      @input="(event) => setHex((event.target as HTMLInputElement).value)"
    />
    <div class="color-channels">
      <NumberField
        v-for="(channel, index) in channels"
        :key="index"
        :model-value="channel"
        :step="0.005"
        :disabled="disabled"
        :title="['r', 'g', 'b', 'a'][index]"
        @update:model-value="(value) => setChannel(index, value)"
      />
    </div>
    <input
      v-if="hasAlpha"
      class="color-alpha"
      type="range"
      min="0"
      max="1"
      step="0.01"
      :value="modelValue[3]"
      :disabled="disabled"
      title="alpha"
      @input="(event) => setChannel(3, Number((event.target as HTMLInputElement).value))"
    />
  </div>
</template>
