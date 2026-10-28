<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { AlertCircle, HelpCircle, X } from "lucide-vue-next";

const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    title: string;
    message: string;
    hasInput?: boolean;
    defaultValue?: string;
    placeholder?: string;
    confirmText?: string;
    cancelText?: string;
    danger?: boolean;
  }>(),
  {
    hasInput: false,
    defaultValue: "",
    placeholder: "",
    confirmText: "确定",
    cancelText: "取消",
    danger: false,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
  (e: "confirm", value?: string): void;
  (e: "cancel"): void;
}>();

const inputVal = ref("");
const inputEl = ref<HTMLInputElement | null>(null);

watch(
  () => props.modelValue,
  async (open) => {
    if (open) {
      inputVal.value = props.defaultValue;
      await nextTick();
      if (props.hasInput && inputEl.value) {
        inputEl.value.focus();
        inputEl.value.select();
      }
    }
  },
);

function confirm() {
  emit("confirm", props.hasInput ? inputVal.value : undefined);
  emit("update:modelValue", false);
}

function cancel() {
  emit("cancel");
  emit("update:modelValue", false);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Enter") {
    event.preventDefault();
    confirm();
  } else if (event.key === "Escape") {
    event.preventDefault();
    cancel();
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="editor-modal-backdrop"
      tabindex="-1"
      @keydown="onKeydown"
      @pointerdown.self="cancel"
    >
      <div class="editor-modal-dialog" @pointerdown.stop>
        <div class="editor-modal-header">
          <AlertCircle v-if="danger" :size="15" class="modal-icon-danger" />
          <HelpCircle v-else :size="15" class="modal-icon-info" />
          <span class="modal-title">{{ title }}</span>
          <span class="spacer" />
          <button class="ghost icon close-btn" @click="cancel">
            <X :size="13" />
          </button>
        </div>

        <div class="editor-modal-body">
          <div class="editor-modal-message">{{ message }}</div>
          <div v-if="hasInput" class="editor-modal-input-wrap">
            <input
              ref="inputEl"
              v-model="inputVal"
              type="text"
              class="editor-modal-input"
              :placeholder="placeholder"
              @keydown.enter.prevent="confirm"
            />
          </div>
        </div>

        <div class="editor-modal-footer">
          <button class="ghost" @click="cancel">{{ cancelText }}</button>
          <button
            :class="danger ? 'danger' : 'primary'"
            @click="confirm"
          >
            {{ confirmText }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.editor-modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.65);
  backdrop-filter: blur(2px);
  z-index: 99995;
  display: flex;
  align-items: center;
  justify-content: center;
  outline: none;
}

.editor-modal-dialog {
  width: 400px;
  max-width: 90vw;
  background: var(--panel-raised, #2e2e2e);
  border: 1px solid var(--border-strong, #484848);
  border-radius: 4px;
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.75);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  animation: modal-enter 0.12s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes modal-enter {
  from {
    opacity: 0;
    transform: scale(0.96) translateY(-4px);
  }
  to {
    opacity: 1;
    transform: scale(1) translateY(0);
  }
}

.editor-modal-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  background: var(--chrome, #222222);
  border-bottom: 1px solid var(--border, #1a1a1a);
  font-weight: 600;
  font-size: 12px;
  color: var(--text-strong, #ffffff);
}

.modal-icon-danger {
  color: var(--danger, #f44747);
  flex-shrink: 0;
}

.modal-icon-info {
  color: var(--accent-alt, #569cd6);
  flex-shrink: 0;
}

.modal-title {
  flex: 1 1 auto;
}

.close-btn {
  width: 18px;
  height: 18px;
  color: var(--muted, #767676);
}
.close-btn:hover {
  color: #ffffff;
}

.editor-modal-body {
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.editor-modal-message {
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--text, #c4c4c4);
  white-space: pre-wrap;
  word-break: break-word;
}

.editor-modal-input-wrap {
  margin-top: 4px;
}

.editor-modal-input {
  width: 100%;
  height: 24px;
  padding: 2px 8px;
  font-size: 12px;
  background: var(--bg-sunken, #181818);
  border: 1px solid var(--border-strong, #3a3a3a);
  border-radius: 2px;
  color: #ffffff;
  outline: none;
}

.editor-modal-input:focus {
  border-color: var(--accent, #3a75c4);
  box-shadow: 0 0 0 1px var(--accent, #3a75c4);
}

.editor-modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 8px 14px;
  background: var(--panel, #282828);
  border-top: 1px solid var(--border, #1a1a1a);
}

.editor-modal-footer button {
  min-width: 68px;
  height: 24px;
  font-size: 11px;
}
</style>
