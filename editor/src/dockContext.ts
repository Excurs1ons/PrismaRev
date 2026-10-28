import type { InjectionKey, Ref } from "vue";
import type { DockZone } from "./dock";

/** 当前指针悬停的停靠目标。 */
export interface DockHover {
  leafId: string;
  zone: DockZone;
  targetIndex?: number;
}

/** DockLayout 提供给递归 DockNode 的操作集合。 */
export interface DockContext {
  dragPanel: Ref<string | null>;
  hover: Ref<DockHover | null>;
  titleOf: (id: string) => string;
  select: (leafId: string, tab: string) => void;
  close: (tab: string) => void;
  resize: (splitId: string, ratio: number) => void;
  drop: (leafId: string, zone: DockZone, tab: string, targetIndex?: number) => void;
  /** 从标签页按下开始一次指针拖拽（不依赖 HTML5 drag-and-drop）。 */
  startDrag: (tab: string, event: PointerEvent) => void;
}

export const DOCK_KEY: InjectionKey<DockContext> = Symbol("prisma-dock");
