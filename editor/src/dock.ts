/**
 * Docking 布局树。
 *
 * 节点只有两种：
 * - `leaf`：一个面板容器，里面可以有多个 tab，同一时间只显示 active。
 * - `split`：水平/垂直二分，`ratio` 是前半部分占的比例。
 *
 * 调整一个 splitter 只改 `ratio`，所以同一侧所有对齐的面板会一起变尺寸；
 * 这也是 ImGui docking 的核心行为。
 */

export type DockDirection = "row" | "column";
export type DockZone = "center" | "left" | "right" | "top" | "bottom";
/** 主窗口三边（顶边是工具栏，不做停靠目标）。 */
export type DockEdge = "left" | "right" | "bottom";

export interface DockLeaf {
  type: "leaf";
  id: string;
  tabs: string[];
  active: string;
}

export interface DockSplit {
  type: "split";
  id: string;
  dir: DockDirection;
  ratio: number;
  a: DockNode;
  b: DockNode;
}

export type DockNode = DockLeaf | DockSplit;

/** 脱离 dock tree 的自由浮动面板。 */
export interface FloatPanel {
  tab: string;
  x: number;
  y: number;
  width: number;
  height: number;
  z: number;
}

export const MIN_FLOAT_WIDTH = 220;
export const MIN_FLOAT_HEIGHT = 140;

export type ResizeDirection = "n" | "s" | "e" | "w" | "nw" | "ne" | "sw" | "se";

export interface FloatRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * 8 方向调整浮动窗口大小与位置。
 *
 * 保证：
 * - 宽度不小于 `MIN_FLOAT_WIDTH`，高度不小于 `MIN_FLOAT_HEIGHT`；
 * - 拖拽上/左边框时，相对锚点（下/右边框）固定；
 * - 拖拽下/右边框时，相对锚点（上/左边框）固定；
 * - 视口边界约束：不超出视口边缘（左/右/底边留 margin，顶边不低于工具栏 36px）。
 */
export function resizeFloatRect(
  initial: FloatRect,
  dir: ResizeDirection,
  dx: number,
  dy: number,
  viewportWidth: number,
  viewportHeight: number,
): FloatRect {
  const margin = 8;
  const topLimit = 36;
  const bottomLimit = Math.max(topLimit + MIN_FLOAT_HEIGHT, viewportHeight - margin);
  const rightLimit = Math.max(margin + MIN_FLOAT_WIDTH, viewportWidth - margin);

  let x = initial.x;
  let y = initial.y;
  let width = initial.width;
  let height = initial.height;

  // 水平调整
  if (dir.includes("e")) {
    const maxWidth = Math.max(MIN_FLOAT_WIDTH, rightLimit - initial.x);
    width = Math.min(maxWidth, Math.max(MIN_FLOAT_WIDTH, initial.width + dx));
  } else if (dir.includes("w")) {
    const maxX = initial.x + initial.width - MIN_FLOAT_WIDTH;
    const requestedX = initial.x + dx;
    x = Math.max(margin, Math.min(maxX, requestedX));
    width = initial.x + initial.width - x;
  }

  // 垂直调整
  if (dir.includes("s")) {
    const maxHeight = Math.max(MIN_FLOAT_HEIGHT, bottomLimit - initial.y);
    height = Math.min(maxHeight, Math.max(MIN_FLOAT_HEIGHT, initial.height + dy));
  } else if (dir.includes("n")) {
    const maxY = initial.y + initial.height - MIN_FLOAT_HEIGHT;
    const requestedY = initial.y + dy;
    y = Math.max(topLimit, Math.min(maxY, requestedY));
    height = initial.y + initial.height - y;
  }

  return { x, y, width, height };
}

export interface SnapResult {
  x: number;
  y: number;
  snapLines: Array<{
    type: "vertical" | "horizontal";
    coord: number;
  }>;
}

export const SNAP_THRESHOLD = 10;

/**
 * 浮动窗口磁吸计算：
 * 与视口边缘（左、右、顶、底）以及其他浮动面板的四边、中心线、级联偏移进行对齐或紧贴吸附。
 */
export function snapFloatPosition(
  panel: { x: number; y: number; width: number; height: number },
  otherPanels: Array<{ x: number; y: number; width: number; height: number }>,
  viewportWidth: number,
  viewportHeight: number,
  threshold = SNAP_THRESHOLD,
): SnapResult {
  const margin = 8;
  const topLimit = 36;
  const bottomLimit = Math.max(topLimit, viewportHeight - panel.height - 24);
  const rightLimit = Math.max(margin, viewportWidth - panel.width - margin);

  let snappedX = panel.x;
  let snappedY = panel.y;
  let bestDistX = threshold + 1;
  let bestDistY = threshold + 1;
  const snapLines: Array<{ type: "vertical" | "horizontal"; coord: number }> = [];

  // 1. 视口边缘候选吸附点
  const xCandidates: Array<{ target: number; line: number }> = [
    { target: margin, line: margin },
    { target: rightLimit, line: viewportWidth - margin },
  ];
  const yCandidates: Array<{ target: number; line: number }> = [
    { target: topLimit, line: topLimit },
    { target: bottomLimit, line: viewportHeight - 24 },
  ];

  // 2. 其它浮动面板的边缘、居中与堆叠级联候选
  for (const other of otherPanels) {
    // 自身左边与其他面板左边对齐
    xCandidates.push({ target: other.x, line: other.x });
    // 自身右边与其他面板右边对齐
    xCandidates.push({ target: other.x + other.width - panel.width, line: other.x + other.width });
    // 自身水平居中与其他面板居中对齐
    xCandidates.push({
      target: other.x + (other.width - panel.width) * 0.5,
      line: other.x + other.width * 0.5,
    });
    // 级联层叠对齐 (+28px 标题栏偏移)
    xCandidates.push({ target: other.x + 28, line: other.x + 28 });
    // 紧贴在其他面板右侧
    xCandidates.push({ target: other.x + other.width, line: other.x + other.width });
    // 紧贴在其他面板左侧
    xCandidates.push({ target: other.x - panel.width, line: other.x });

    // 自身顶边与其他面板顶边对齐
    yCandidates.push({ target: other.y, line: other.y });
    // 自身底边与其他面板底边对齐
    yCandidates.push({ target: other.y + other.height - panel.height, line: other.y + other.height });
    // 自身垂直居中与其他面板居中对齐
    yCandidates.push({
      target: other.y + (other.height - panel.height) * 0.5,
      line: other.y + other.height * 0.5,
    });
    // 级联层叠对齐 (+28px 标题栏偏移)
    yCandidates.push({ target: other.y + 28, line: other.y + 28 });
    // 紧贴在其他面板下方
    yCandidates.push({ target: other.y + other.height, line: other.y + other.height });
    // 紧贴在其他面板上方
    yCandidates.push({ target: other.y - panel.height, line: other.y });
  }

  let chosenLineX: number | null = null;
  for (const { target, line } of xCandidates) {
    const dist = Math.abs(panel.x - target);
    if (dist <= threshold && dist < bestDistX) {
      bestDistX = dist;
      snappedX = target;
      chosenLineX = line;
    }
  }
  if (chosenLineX !== null) {
    snapLines.push({ type: "vertical", coord: chosenLineX });
  }

  let chosenLineY: number | null = null;
  for (const { target, line } of yCandidates) {
    const dist = Math.abs(panel.y - target);
    if (dist <= threshold && dist < bestDistY) {
      bestDistY = dist;
      snappedY = target;
      chosenLineY = line;
    }
  }
  if (chosenLineY !== null) {
    snapLines.push({ type: "horizontal", coord: chosenLineY });
  }

  return { x: snappedX, y: snappedY, snapLines };
}

const MIN_RATIO = 0.12;
const MAX_RATIO = 0.88;

let seq = 0;

function uid(prefix: string): string {
  seq += 1;
  return `${prefix}${Date.now().toString(36).slice(-4)}${seq}`;
}

export function leaf(tabs: string[], active = tabs[0] ?? ""): DockLeaf {
  return { type: "leaf", id: uid("leaf-"), tabs: [...tabs], active };
}

export function split(
  dir: DockDirection,
  a: DockNode,
  b: DockNode,
  ratio = 0.5,
): DockSplit {
  return { type: "split", id: uid("split-"), dir, ratio: clampRatio(ratio), a, b };
}

export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0.5;
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
}

/** 让浮动面板始终留在视口内。 */
export function clampFloatPanel(
  panel: FloatPanel,
  viewportWidth: number,
  viewportHeight: number,
): FloatPanel {
  const margin = 8;
  const maxWidth = Math.max(MIN_FLOAT_WIDTH, viewportWidth - margin * 2);
  const maxHeight = Math.max(MIN_FLOAT_HEIGHT, viewportHeight - 48);
  const width = Math.min(Math.max(panel.width, MIN_FLOAT_WIDTH), maxWidth);
  const height = Math.min(Math.max(panel.height, MIN_FLOAT_HEIGHT), maxHeight);
  return {
    ...panel,
    width,
    height,
    x: Math.min(Math.max(panel.x, margin), Math.max(margin, viewportWidth - width - margin)),
    y: Math.min(Math.max(panel.y, 36), Math.max(36, viewportHeight - height - margin)),
  };
}

/** 清理持久化的浮动面板（未知 tab / 重复 tab / 非数字坐标）。 */
export function sanitizeFloating(value: unknown, knownTabs: string[]): FloatPanel[] {
  if (!Array.isArray(value)) return [];
  const known = new Set(knownTabs);
  const seen = new Set<string>();
  const result: FloatPanel[] = [];
  value.forEach((item, index) => {
    if (!item || typeof item !== "object") return;
    const raw = item as Record<string, unknown>;
    const tab = typeof raw.tab === "string" ? raw.tab : "";
    if (!known.has(tab) || seen.has(tab)) return;
    seen.add(tab);
    const number = (key: string, fallback: number) =>
      typeof raw[key] === "number" && Number.isFinite(raw[key]) ? (raw[key] as number) : fallback;
    result.push(
      clampFloatPanel(
        {
          tab,
          x: number("x", 40 + index * 24),
          y: number("y", 60 + index * 24),
          width: number("width", 300),
          height: number("height", 340),
          z: number("z", index + 1),
        },
        typeof window === "undefined" ? 1280 : window.innerWidth,
        typeof window === "undefined" ? 800 : window.innerHeight,
      ),
    );
  });
  return result;
}

/** 默认：左侧 文档/层级/资产，右侧检查器，底部输出（输出默认隐藏）。 */
export function defaultDock(): DockNode {
  const assets = leaf(["assets"]);
  const hierarchy = split("column", leaf(["hierarchy"]), assets, 0.55);
  const left = split("column", leaf(["docs"]), hierarchy, 0.3);
  // 中间是视口（场景 / 游戏视图同 leaf，标签切换），右侧检查器。
  const viewport = leaf(["sceneView", "gameView"]);
  const main = split("row", viewport, leaf(["inspector"]), 0.76);
  return split("row", left, main, 0.19);
}

/** 经典 2 by 3 布局：左视口（Scene+Game 上下），中层级/资产，右检查器。 */
export function twoByThreeDock(): DockNode {
  const leftViewports = split("column", leaf(["sceneView"]), leaf(["gameView"]), 0.5);
  const midTrees = split("column", leaf(["hierarchy", "docs"]), leaf(["assets", "log"]), 0.55);
  const leftAndMid = split("row", leftViewports, midTrees, 0.7);
  return split("row", leftAndMid, leaf(["inspector"]), 0.78);
}

/** 经典 Tall 布局：左侧上下视口，右侧单列竖排（层级/资产/检查器）。 */
export function tallDock(): DockNode {
  const leftViewports = split("column", leaf(["sceneView"]), leaf(["gameView"]), 0.5);
  const rightColumn = split("column", leaf(["hierarchy", "assets"]), leaf(["inspector"]), 0.45);
  return split("row", leftViewports, rightColumn, 0.72);
}

/** 经典 Wide 布局：上方宽幅视口，下方横排面板。 */
export function wideDock(): DockNode {
  const topViewports = split("row", leaf(["sceneView"]), leaf(["gameView"]), 0.5);
  const bottomPanels = split(
    "row",
    leaf(["hierarchy", "docs"]),
    split("row", leaf(["assets", "log"]), leaf(["inspector"]), 0.55),
    0.28,
  );
  return split("column", topViewports, bottomPanels, 0.65);
}

export const LAYOUT_PRESETS: Array<{ id: string; label: string; factory: () => DockNode }> = [
  { id: "default", label: "Default", factory: defaultDock },
  { id: "2by3", label: "2 by 3", factory: twoByThreeDock },
  { id: "tall", label: "Tall", factory: tallDock },
  { id: "wide", label: "Wide", factory: wideDock },
];

export function cloneNode(node: DockNode): DockNode {
  return node.type === "leaf"
    ? { ...node, tabs: [...node.tabs] }
    : { ...node, a: cloneNode(node.a), b: cloneNode(node.b) };
}

export function collectTabs(node: DockNode): string[] {
  return node.type === "leaf"
    ? [...node.tabs]
    : [...collectTabs(node.a), ...collectTabs(node.b)];
}

export function findLeaf(node: DockNode, tab: string): DockLeaf | null {
  if (node.type === "leaf") return node.tabs.includes(tab) ? node : null;
  return findLeaf(node.a, tab) ?? findLeaf(node.b, tab);
}

export function firstLeaf(node: DockNode): DockLeaf | null {
  return node.type === "leaf" ? node : firstLeaf(node.a) ?? firstLeaf(node.b);
}

/** 从树中移除一个 tab；空 leaf 被剪掉，只剩一个孩子的 split 会被折叠。 */
export function removePanel(node: DockNode, tab: string): DockNode | null {
  if (node.type === "leaf") {
    if (!node.tabs.includes(tab)) return node;
    const tabs = node.tabs.filter((item) => item !== tab);
    if (!tabs.length) return null;
    const active = tabs.includes(node.active) ? node.active : tabs[0];
    return { ...node, tabs, active };
  }

  const a = removePanel(node.a, tab);
  const b = removePanel(node.b, tab);
  if (!a) return b;
  if (!b) return a;
  return { ...node, a, b };
}

/** 切换某个 leaf 的 active tab。 */
export function activateTab(node: DockNode, leafId: string, tab: string): DockNode {
  if (node.type === "leaf") {
    if (node.id !== leafId || !node.tabs.includes(tab)) return node;
    return { ...node, active: tab };
  }
  return { ...node, a: activateTab(node.a, leafId, tab), b: activateTab(node.b, leafId, tab) };
}

/** 更新 split 的比例（按 split id 定位）。 */
export function setRatio(node: DockNode, splitId: string, ratio: number): DockNode {
  if (node.type === "leaf") return node;
  if (node.id === splitId) return { ...node, ratio: clampRatio(ratio) };
  return {
    ...node,
    a: setRatio(node.a, splitId, ratio),
    b: setRatio(node.b, splitId, ratio),
  };
}

/**
 * 把一个面板放到目标 leaf 的某个区域。
 * - center：变成目标 leaf 的一个 tab；
 * - left/right/top/bottom：在目标旁边新开一个 leaf。
 */
export function insertPanel(
  root: DockNode,
  targetLeafId: string,
  tab: string,
  zone: DockZone,
  ratio = 0.5,
  targetIndex?: number,
): DockNode {
  const target = findLeafById(root, targetLeafId);
  if (!target) return root;

  if (zone === "center") {
    if (target.tabs.includes(tab)) {
      // 在同一 leaf 内部拖拽标签页：支持重新排序
      const currentIndex = target.tabs.indexOf(tab);
      const filtered = target.tabs.filter((item) => item !== tab);
      let insertAt: number;
      if (targetIndex !== undefined) {
        // 当从左向右拖拽跨越标签时，原 targetIndex 位于移除项之后；
        // 在 filtered 列表中需要减 1 才能精确对齐视觉插入标记。
        const adjusted = targetIndex > currentIndex ? targetIndex - 1 : targetIndex;
        insertAt = Math.max(0, Math.min(adjusted, filtered.length));
      } else {
        insertAt = filtered.length;
      }
      filtered.splice(insertAt, 0, tab);
      return replaceLeaf(root, targetLeafId, {
        ...target,
        tabs: filtered,
        active: tab,
      });
    }
    const detached = removePanel(root, tab) ?? root;
    const nextTarget = findLeafById(detached, targetLeafId);
    if (!nextTarget) {
      // 目标 leaf 因为拖走的是它唯一的面板而消失了：直接放回原位。
      return detached;
    }
    const nextTabs = [...nextTarget.tabs];
    const insertAt =
      targetIndex !== undefined
        ? Math.max(0, Math.min(targetIndex, nextTabs.length))
        : nextTabs.length;
    nextTabs.splice(insertAt, 0, tab);
    return replaceLeaf(detached, targetLeafId, {
      ...nextTarget,
      tabs: nextTabs,
      active: tab,
    });
  }

  // 先把 tab 从旧位置摘掉；如果目标 leaf 因此消失，说明拖的是目标 leaf
  // 自己的唯一标签页——对自身做边缘分屏只会复制出第二份，直接不处理。
  const next = removePanel(root, tab);
  if (!next) return root;
  const nextTarget = findLeafById(next, targetLeafId);
  if (!nextTarget) return root;

  const placed = leaf([tab]);
  const dir: DockDirection = zone === "left" || zone === "right" ? "row" : "column";
  const first = zone === "left" || zone === "top" ? placed : nextTarget;
  const second = zone === "left" || zone === "top" ? nextTarget : placed;
  const node = split(dir, first, second, zone === "left" || zone === "top" ? ratio : 1 - ratio);
  return replaceLeaf(next, targetLeafId, node);
}

/**
 * 把面板停靠到**主窗口**的左/右/底边（新开一整列或一整行）。
 *
 * 与 `insertPanel` 的区别：目标不是某个 leaf 的边缘，而是整棵 dock tree
 * 的外侧。若该 tab 本来就是整棵树唯一面板，则保持原样（已经占满窗口）。
 */
export function dockPanelAtEdge(
  root: DockNode | null,
  tab: string,
  edge: DockEdge,
  ratio = 0.25,
): DockNode {
  const next = root ? removePanel(root, tab) : null;
  if (!next) return root ?? leaf([tab]);

  const placed = leaf([tab]);
  if (edge === "left") return split("row", placed, next, ratio);
  if (edge === "right") return split("row", next, placed, 1 - ratio);
  return split("column", next, placed, 1 - ratio);
}

/** 把布局从 `oldLeafId` 换成任意节点。 */
function replaceLeaf(node: DockNode, leafId: string, replacement: DockNode): DockNode {
  if (node.type === "leaf") return node.id === leafId ? replacement : node;
  return {
    ...node,
    a: replaceLeaf(node.a, leafId, replacement),
    b: replaceLeaf(node.b, leafId, replacement),
  };
}

export function findLeafById(node: DockNode, leafId: string): DockLeaf | null {
  if (node.type === "leaf") return node.id === leafId ? node : null;
  return findLeafById(node.a, leafId) ?? findLeafById(node.b, leafId);
}

/** 给「显示面板」菜单用：把面板放回一个合理的锚点旁边。 */
export function showPanel(root: DockNode, tab: string): DockNode {
  if (collectTabs(root).includes(tab)) {
    const target = findLeaf(root, tab);
    return target ? activateTab(root, target.id, tab) : root;
  }

  if (tab === "log") {
    return split("column", root, leaf([tab]), 0.76);
  }

  // 工具/设置/渲染图等辅助窗口默认作为右侧面板的 tab，避免每开一个
  // 就新切一列；用户仍可把 tab 拖出来浮动或重新分屏。
  const utilityTabs = new Set([
    "renderGraph",
    "projectSettings",
    "preferences",
    "build",
    "tools",
    "shortcuts",
    "about",
  ]);
  if (utilityTabs.has(tab)) {
    for (const host of ["inspector", ...utilityTabs]) {
      const target = findLeaf(root, host);
      if (target) return insertPanel(root, target.id, tab, "center");
    }
  }

  const anchors: Record<string, Array<[string, DockZone]>> = {
    docs: [
      ["hierarchy", "top"],
      ["assets", "top"],
      ["inspector", "left"],
    ],
    hierarchy: [
      ["docs", "bottom"],
      ["assets", "top"],
    ],
    assets: [
      ["hierarchy", "bottom"],
      ["docs", "bottom"],
    ],
    inspector: [
      ["docs", "right"],
      ["hierarchy", "right"],
      ["assets", "right"],
    ],
    shortcuts: [["inspector", "center"]],
    about: [["inspector", "center"]],
  };

  for (const [anchor, zone] of anchors[tab] ?? []) {
    const target = findLeaf(root, anchor);
    if (target) return insertPanel(root, target.id, tab, zone, 0.35);
  }

  const fallback = firstLeaf(root);
  if (!fallback) return leaf([tab]);
  return insertPanel(root, fallback.id, tab, "center");
}

/** 清理持久化数据：只保留已知面板，去掉空 leaf / 单孩子 split。 */
export function sanitizeDock(node: unknown, knownTabs: string[]): DockNode | null {
  const known = new Set(knownTabs);
  const clean = (value: unknown): DockNode | null => {
    if (!value || typeof value !== "object") return null;
    const raw = value as {
      type?: unknown;
      id?: unknown;
      tabs?: unknown;
      active?: unknown;
      dir?: unknown;
      ratio?: unknown;
      a?: unknown;
      b?: unknown;
    };
    if (raw.type === "leaf") {
      const tabs = Array.isArray(raw.tabs)
        ? raw.tabs.filter((tab): tab is string => typeof tab === "string" && known.has(tab))
        : [];
      if (!tabs.length) return null;
      const active = typeof raw.active === "string" && tabs.includes(raw.active)
        ? raw.active
        : tabs[0];
      return {
        type: "leaf",
        id: typeof raw.id === "string" && raw.id ? raw.id : uid("leaf-"),
        tabs,
        active,
      };
    }
    if (raw.type === "split") {
      const a = clean(raw.a);
      const b = clean(raw.b);
      if (!a) return b;
      if (!b) return a;
      return {
        type: "split",
        id: typeof raw.id === "string" && raw.id ? raw.id : uid("split-"),
        dir: raw.dir === "column" ? "column" : "row",
        ratio: clampRatio(Number(raw.ratio)),
        a,
        b,
      };
    }
    return null;
  };
  return clean(node);
}
