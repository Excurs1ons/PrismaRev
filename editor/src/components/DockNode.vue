<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { X } from "lucide-vue-next";
import type { DockNode } from "../dock";
import { DOCK_KEY, type DockContext } from "../dockContext";

defineOptions({ name: "DockNode" });

const props = defineProps<{ node: DockNode }>();
const injected = inject<DockContext>(DOCK_KEY);
if (!injected) throw new Error("DockNode must be rendered inside DockLayout");
const ctx: DockContext = injected;

const splitEl = ref<HTMLElement | null>(null);
const hoverZone = computed(() => {
  const target = ctx.hover.value;
  return target && target.leafId === props.node.id ? target.zone : null;
});

function startResize(event: PointerEvent) {
  if (props.node.type !== "split") return;
  const el = splitEl.value;
  if (!el) return;
  event.preventDefault();
  const target = event.currentTarget as HTMLElement | null;
  target?.setPointerCapture?.(event.pointerId);
  const horizontal = props.node.dir === "row";
  const rect = el.getBoundingClientRect();
  const splitId = props.node.id;

  const move = (ev: PointerEvent) => {
    const ratio = horizontal
      ? (ev.clientX - rect.left) / Math.max(1, rect.width)
      : (ev.clientY - rect.top) / Math.max(1, rect.height);
    ctx.resize(splitId, ratio);
  };
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", stop);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", stop);
  document.body.style.cursor = horizontal ? "col-resize" : "row-resize";
  document.body.style.userSelect = "none";
}

</script>

<template>
  <div
    v-if="node.type === 'split'"
    ref="splitEl"
    class="dock-split"
    :class="node.dir === 'row' ? 'dock-row' : 'dock-col'"
  >
    <div class="dock-pane" :style="{ flexGrow: node.ratio, flexBasis: '0%' }">
      <DockNode :node="node.a">
        <template v-for="(_, name) in $slots" #[name]>
          <slot :name="name" />
        </template>
      </DockNode>
    </div>
    <div
      class="dock-splitter"
      :class="node.dir === 'row' ? 'dock-row' : 'dock-col'"
      title="拖动调整相邻面板尺寸"
      @pointerdown="startResize"
    />
    <div class="dock-pane" :style="{ flexGrow: 1 - node.ratio, flexBasis: '0%' }">
      <DockNode :node="node.b">
        <template v-for="(_, name) in $slots" #[name]>
          <slot :name="name" />
        </template>
      </DockNode>
    </div>
  </div>

  <div v-else class="dock-leaf" :data-leaf-id="node.id">
    <div class="dock-tabs">
      <div
        v-for="tab in node.tabs"
        :key="tab"
        class="dock-tab"
        :class="{ active: tab === node.active }"
        :title="`${ctx.titleOf(tab)}（拖动可停靠；按住 Alt 拖出为浮动窗口）`"
        @click="ctx.select(node.id, tab)"
        @pointerdown="ctx.startDrag(tab, $event)"
      >
        <span class="dock-tab-title">{{ ctx.titleOf(tab) }}</span>
        <button
          class="dock-tab-close"
          title="关闭面板"
          @pointerdown.stop
          @click.stop="ctx.close(tab)"
        >
          <X :size="12" />
        </button>
      </div>
    </div>

    <div class="dock-body">
      <slot :name="node.active" />
    </div>

    <div v-if="hoverZone" class="dock-drop" :class="`zone-${hoverZone}`">
      <span>{{ hoverZone === "center" ? "合并为标签页" : "停靠" }}</span>
    </div>
  </div>
</template>
