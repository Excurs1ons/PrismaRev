<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Check, ChevronDown } from "lucide-vue-next";

export interface DropdownOption {
  value: string | number;
  label: string;
  badge?: string | number;
}

const props = withDefaults(
  defineProps<{
    modelValue: string | number;
    options: DropdownOption[];
    title?: string;
    disabled?: boolean;
    block?: boolean;
  }>(),
  {
    block: false,
    title: undefined,
    disabled: false,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: string | number): void;
  (e: "change", value: string | number): void;
}>();

const isOpen = ref(false);
const rootEl = ref<HTMLElement | null>(null);

const currentOption = computed(() =>
  props.options.find((opt) => opt.value === props.modelValue),
);

const currentLabel = computed(() => currentOption.value?.label ?? String(props.modelValue));

const menuStyle = computed(() => {
  if (!rootEl.value) return {};
  const rect = rootEl.value.getBoundingClientRect();
  const screenW = typeof window !== "undefined" ? window.innerWidth : 1920;
  const menuW = Math.max(120, rect.width);
  let left = rect.left;
  if (left + menuW > screenW - 8) {
    left = Math.max(8, screenW - menuW - 8);
  }
  return {
    left: `${left}px`,
    top: `${rect.bottom + 2}px`,
    minWidth: `${menuW}px`,
  };
});

function toggle() {
  if (props.disabled) return;
  isOpen.value = !isOpen.value;
}

function select(val: string | number) {
  isOpen.value = false;
  emit("update:modelValue", val);
  emit("change", val);
}

function handleClickOutside(event: MouseEvent) {
  if (rootEl.value && !rootEl.value.contains(event.target as Node)) {
    isOpen.value = false;
  }
}

onMounted(() => {
  window.addEventListener("pointerdown", handleClickOutside);
  window.addEventListener("blur", () => (isOpen.value = false));
});

onUnmounted(() => {
  window.removeEventListener("pointerdown", handleClickOutside);
});
</script>

<template>
  <div ref="rootEl" class="editor-dropdown" :class="{ block }" :title="title">
    <button
      type="button"
      class="editor-dropdown-trigger"
      :class="{ open: isOpen, disabled }"
      :disabled="disabled"
      @click="toggle"
    >
      <span class="dropdown-trigger-label">{{ currentLabel }}</span>
      <ChevronDown :size="12" class="dropdown-chevron" />
    </button>
    <Teleport to="body">
      <div
        v-if="isOpen && rootEl"
        class="editor-dropdown-menu"
        :style="menuStyle"
        @pointerdown.stop
      >
        <div
          v-for="opt in options"
          :key="opt.value"
          class="editor-dropdown-item"
          :class="{ active: modelValue === opt.value }"
          @click="select(opt.value)"
        >
          <Check v-if="modelValue === opt.value" :size="12" class="dropdown-check" />
          <span v-else class="dropdown-check-spacer" />
          <span class="dropdown-item-label">{{ opt.label }}</span>
          <span v-if="opt.badge !== undefined" class="dropdown-item-badge">{{ opt.badge }}</span>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.editor-dropdown {
  position: relative;
  display: inline-block;
  user-select: none;
}

.editor-dropdown.block {
  display: block;
  width: 100%;
}

.editor-dropdown-trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  height: 22px;
  padding: 1px 7px;
  font-size: 11px;
  background: var(--bg-sunken, #181818);
  border: 1px solid var(--border-strong, #353535);
  border-radius: 2px;
  color: var(--text-strong, #ffffff);
  cursor: pointer;
  white-space: nowrap;
}

.editor-dropdown.block .editor-dropdown-trigger {
  width: 100%;
}

.editor-dropdown-trigger:hover:not(:disabled) {
  background: #2a2a2a;
  border-color: #555555;
}

.editor-dropdown-trigger.open {
  border-color: var(--accent, #3a75c4);
  box-shadow: 0 0 0 1px var(--accent, #3a75c4);
}

.editor-dropdown-trigger:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.dropdown-trigger-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dropdown-chevron {
  color: var(--muted, #767676);
  flex-shrink: 0;
}

.editor-dropdown-menu {
  position: fixed;
  background: var(--panel-raised, #2e2e2e);
  border: 1px solid var(--border-strong, #444444);
  border-radius: 3px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.65);
  z-index: 99990;
  padding: 3px 0;
  max-height: 260px;
  overflow-y: auto;
}

.editor-dropdown-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  font-size: 11px;
  color: var(--text, #c4c4c4);
  cursor: pointer;
  white-space: nowrap;
}

.editor-dropdown-item:hover {
  background: var(--accent, #3a75c4);
  color: #ffffff;
}

.editor-dropdown-item.active {
  color: var(--text-strong, #ffffff);
  font-weight: 500;
}

.dropdown-check {
  color: var(--accent-alt, #569cd6);
  flex-shrink: 0;
}

.editor-dropdown-item:hover .dropdown-check {
  color: #ffffff;
}

.dropdown-check-spacer {
  width: 12px;
  flex-shrink: 0;
}

.dropdown-item-label {
  flex: 1 1 auto;
}

.dropdown-item-badge {
  font-size: 10px;
  color: var(--muted);
  background: var(--panel);
  border-radius: 2px;
  padding: 0 4px;
}
.editor-dropdown-item:hover .dropdown-item-badge {
  color: #ffffff;
  background: rgba(0, 0, 0, 0.25);
}
</style>
