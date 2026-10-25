/**
 * 编辑器界面主题（纯前端，保存在本机 localStorage）。
 *
 * 主题只描述**颜色**：每个 token 对应 `style.css` 里同名 CSS 变量，
 * `applyTheme` 把它们写到 `document.documentElement` 上，所以切换主题不需要
 * 重新渲染组件，也不会碰项目里的任何文件。
 */

/** 所有可主题化的颜色 token（一套主题 = 一套完整取值）。 */
export interface ThemeColors {
  /* 背景与表面 */
  bg: string;
  bgSunken: string;
  panel: string;
  panelAlt: string;
  panelRaised: string;
  chrome: string;
  /* 边框与分隔 */
  border: string;
  borderStrong: string;
  borderSubtle: string;
  /* 文本 */
  text: string;
  textStrong: string;
  textSoft: string;
  muted: string;
  mutedDim: string;
  /* 交互状态 */
  rowHover: string;
  menuHover: string;
  menuActive: string;
  selection: string;
  /* 强调色 */
  accent: string;
  accentSolid: string;
  onAccent: string;
  accentInk: string;
  accentAlt: string;
  /* 状态色 */
  success: string;
  successSoft: string;
  successBg: string;
  successBorder: string;
  danger: string;
  dangerSoft: string;
  dangerBg: string;
  dangerBgSubtle: string;
  dangerSolid: string;
  dangerBorder: string;
  onDanger: string;
  warning: string;
  warningStrong: string;
  warningBorder: string;
  panic: string;
}

export type ThemeTokenKey = keyof ThemeColors;

export interface ThemeToken {
  key: ThemeTokenKey;
  cssVar: string;
  label: string;
}

export interface ThemeTokenGroup {
  id: string;
  label: string;
  tokens: ThemeToken[];
}

/** 属性面板式的分组元数据（顺序即面板里的展示顺序）。 */
export const THEME_TOKEN_GROUPS: ThemeTokenGroup[] = [
  {
    id: "surface",
    label: "背景与表面",
    tokens: [
      { key: "bg", cssVar: "--bg", label: "应用背景" },
      { key: "bgSunken", cssVar: "--bg-sunken", label: "凹陷区域（日志/输入）" },
      { key: "panel", cssVar: "--panel", label: "面板" },
      { key: "panelAlt", cssVar: "--panel-alt", label: "次级面板（卡头/tab）" },
      { key: "panelRaised", cssVar: "--panel-raised", label: "浮层（菜单/弹窗）" },
      { key: "chrome", cssVar: "--chrome", label: "菜单栏/状态栏" },
    ],
  },
  {
    id: "border",
    label: "边框与分隔",
    tokens: [
      { key: "border", cssVar: "--border", label: "边框" },
      { key: "borderStrong", cssVar: "--border-strong", label: "强边框/分隔线" },
      { key: "borderSubtle", cssVar: "--border-subtle", label: "弱边框" },
    ],
  },
  {
    id: "text",
    label: "文本",
    tokens: [
      { key: "text", cssVar: "--text", label: "正文" },
      { key: "textStrong", cssVar: "--text-strong", label: "高亮正文/标题" },
      { key: "textSoft", cssVar: "--text-soft", label: "次级正文" },
      { key: "muted", cssVar: "--muted", label: "弱化文本" },
      { key: "mutedDim", cssVar: "--muted-dim", label: "最弱文本（提示）" },
    ],
  },
  {
    id: "interaction",
    label: "交互状态",
    tokens: [
      { key: "rowHover", cssVar: "--row-hover", label: "行悬停底色" },
      { key: "menuHover", cssVar: "--menu-hover", label: "菜单悬停底色" },
      { key: "menuActive", cssVar: "--menu-active", label: "菜单展开/按下底色" },
      { key: "selection", cssVar: "--selection", label: "选中行底色" },
    ],
  },
  {
    id: "accent",
    label: "强调色",
    tokens: [
      { key: "accent", cssVar: "--accent", label: "强调色" },
      { key: "accentSolid", cssVar: "--accent-solid", label: "实心强调块" },
      { key: "onAccent", cssVar: "--on-accent", label: "实心强调块上的文字" },
      { key: "accentInk", cssVar: "--accent-ink", label: "渐变按钮上的文字" },
      { key: "accentAlt", cssVar: "--accent-alt", label: "第二强调色" },
    ],
  },
  {
    id: "status",
    label: "状态色",
    tokens: [
      { key: "success", cssVar: "--success", label: "成功/运行指示" },
      { key: "successSoft", cssVar: "--success-soft", label: "成功文字" },
      { key: "successBg", cssVar: "--success-bg", label: "成功底色（Play）" },
      { key: "successBorder", cssVar: "--success-border", label: "成功边框" },
      { key: "danger", cssVar: "--danger", label: "危险/错误" },
      { key: "dangerSoft", cssVar: "--danger-soft", label: "危险文字" },
      { key: "dangerBg", cssVar: "--danger-bg", label: "危险底色（Stop）" },
      { key: "dangerBgSubtle", cssVar: "--danger-bg-subtle", label: "危险悬停底色" },
      { key: "dangerSolid", cssVar: "--danger-solid", label: "危险实心块" },
      { key: "dangerBorder", cssVar: "--danger-border", label: "危险边框" },
      { key: "onDanger", cssVar: "--on-danger", label: "危险块上的文字" },
      { key: "warning", cssVar: "--warning", label: "警告" },
      { key: "warningStrong", cssVar: "--warning-strong", label: "警告强调" },
      { key: "warningBorder", cssVar: "--warning-border", label: "警告边框" },
      { key: "panic", cssVar: "--panic", label: "panic 日志" },
    ],
  },
];

export interface ThemePreset {
  id: string;
  label: string;
  description: string;
  mode: "dark" | "light";
  colors: ThemeColors;
}

/** 预设主题。默认项与改造前的编辑器配色保持一致。 */
export const THEME_PRESETS: ThemePreset[] = [
  {
    id: "midnight",
    label: "深空蓝",
    description: "默认 · 冷蓝强调",
    mode: "dark",
    colors: {
      bg: "#0b0e14", bgSunken: "#0d1119", panel: "#121722",
      panelAlt: "#171d2b", panelRaised: "#1d2027", chrome: "#181a1f",
      border: "#232b3d", borderStrong: "#333843", borderSubtle: "#2a2d35",
      text: "#e6ebf5", textStrong: "#ffffff", textSoft: "#cfd8ea",
      muted: "#8b96ad", mutedDim: "#7f8a9e",
      rowHover: "#171d2b", menuHover: "#242832", menuActive: "#2a3242",
      selection: "#1b2333",
      accent: "#6ea8ff", accentSolid: "#2d68c4", onAccent: "#ffffff",
      accentInk: "#06121f", accentAlt: "#5ff0d8",
      success: "#4fbf87", successSoft: "#8fe0b0", successBg: "#255f3b",
      successBorder: "#2f7a4b",
      danger: "#ff6b6b", dangerSoft: "#ffb4b4", dangerBg: "#6a2a2f",
      dangerBgSubtle: "#2a1b1f", dangerSolid: "#8d3038", dangerBorder: "#934049",
      onDanger: "#ffe4e6",
      warning: "#ffcf6b", warningStrong: "#e0a84c", warningBorder: "#7a652f",
      panic: "#ff7ad9",
    },
  },
  {
    id: "violet",
    label: "午夜紫",
    description: "紫罗兰强调 · 粉紫日志",
    mode: "dark",
    colors: {
      bg: "#0b0912", bgSunken: "#0d0b16", panel: "#131020",
      panelAlt: "#191527", panelRaised: "#201b30", chrome: "#171320",
      border: "#2a2440", borderStrong: "#3b3355", borderSubtle: "#241f36",
      text: "#eeeaff", textStrong: "#ffffff", textSoft: "#d4cdf2",
      muted: "#9a92bd", mutedDim: "#7d76a0",
      rowHover: "#191527", menuHover: "#241f38", menuActive: "#2d2748",
      selection: "#211a3a",
      accent: "#b48cff", accentSolid: "#6b46c9", onAccent: "#ffffff",
      accentInk: "#0b0912", accentAlt: "#ff7ad9",
      success: "#5fd39a", successSoft: "#a8f0cd", successBg: "#1f5c40",
      successBorder: "#2f7a55",
      danger: "#ff7a8a", dangerSoft: "#ffc2c9", dangerBg: "#6a2432",
      dangerBgSubtle: "#2a1820", dangerSolid: "#8d2f44", dangerBorder: "#a03a52",
      onDanger: "#ffe6ea",
      warning: "#ffd27a", warningStrong: "#e0a84c", warningBorder: "#7a5f2f",
      panic: "#ff9ae8",
    },
  },
  {
    id: "graphite",
    label: "石墨灰",
    description: "中性灰底 · 琥珀强调",
    mode: "dark",
    colors: {
      bg: "#0e0f11", bgSunken: "#0a0b0d", panel: "#15171a",
      panelAlt: "#1a1c20", panelRaised: "#212429", chrome: "#17191c",
      border: "#2a2d33", borderStrong: "#3a3e46", borderSubtle: "#24262b",
      text: "#e7e9ec", textStrong: "#ffffff", textSoft: "#ccd0d6",
      muted: "#8e949e", mutedDim: "#737982",
      rowHover: "#1a1c20", menuHover: "#26292f", menuActive: "#2f333b",
      selection: "#22262c",
      accent: "#e2b455", accentSolid: "#8a6a18", onAccent: "#ffffff",
      accentInk: "#14120a", accentAlt: "#7fd7c3",
      success: "#58bd86", successSoft: "#9ee6bc", successBg: "#23553a",
      successBorder: "#31754f",
      danger: "#ff7373", dangerSoft: "#ffbcbc", dangerBg: "#5f2a2a",
      dangerBgSubtle: "#2a1b1b", dangerSolid: "#8d3030", dangerBorder: "#a03d3d",
      onDanger: "#ffe6e6",
      warning: "#ffd06b", warningStrong: "#e0a84c", warningBorder: "#7a652f",
      panic: "#ff7ad9",
    },
  },
  {
    id: "forest",
    label: "森林绿",
    description: "墨绿底 · 薄荷强调",
    mode: "dark",
    colors: {
      bg: "#08100b", bgSunken: "#070d09", panel: "#0f1a13",
      panelAlt: "#132117", panelRaised: "#18281d", chrome: "#0e1712",
      border: "#203828", borderStrong: "#2d4d37", borderSubtle: "#1b2f22",
      text: "#e4f2e8", textStrong: "#ffffff", textSoft: "#c6ddcd",
      muted: "#86a291", mutedDim: "#6b8577",
      rowHover: "#132117", menuHover: "#1a2b20", menuActive: "#21382a",
      selection: "#17301f",
      accent: "#6ee7a0", accentSolid: "#237a4a", onAccent: "#ffffff",
      accentInk: "#06120c", accentAlt: "#7fd7c3",
      success: "#4fbf87", successSoft: "#a9f0c6", successBg: "#1f5c3b",
      successBorder: "#2f7a52",
      danger: "#ff7d7d", dangerSoft: "#ffc0c0", dangerBg: "#5e2a2c",
      dangerBgSubtle: "#261a1b", dangerSolid: "#8d3238", dangerBorder: "#a03f45",
      onDanger: "#ffe8e8",
      warning: "#ffd27a", warningStrong: "#e0a84c", warningBorder: "#7a652f",
      panic: "#ff8ce0",
    },
  },
  {
    id: "paper",
    label: "纸白",
    description: "浅色底 · 高可读",
    mode: "light",
    colors: {
      bg: "#f2f4f7", bgSunken: "#e8ebf0", panel: "#ffffff",
      panelAlt: "#f7f8fa", panelRaised: "#ffffff", chrome: "#eef1f5",
      border: "#d6dae1", borderStrong: "#b9bfc9", borderSubtle: "#e3e6eb",
      text: "#20242c", textStrong: "#000000", textSoft: "#4a5261",
      muted: "#6b7383", mutedDim: "#8b93a1",
      rowHover: "#eef1f6", menuHover: "#e9edf4", menuActive: "#dde5f2",
      selection: "#e2ebfa",
      accent: "#1f6feb", accentSolid: "#1f6feb", onAccent: "#ffffff",
      accentInk: "#ffffff", accentAlt: "#0f9d8a",
      success: "#1f8f55", successSoft: "#146b3f", successBg: "#d9f2e4",
      successBorder: "#86cfa8",
      danger: "#d0343f", dangerSoft: "#a3242e", dangerBg: "#fbdadd",
      dangerBgSubtle: "#fdeced", dangerSolid: "#d0343f", dangerBorder: "#e79aa1",
      onDanger: "#ffffff",
      warning: "#b07a12", warningStrong: "#8a5f0a", warningBorder: "#e3c98a",
      panic: "#b13fb0",
    },
  },
];

export const DEFAULT_THEME_PRESET = THEME_PRESETS[0].id;

/** 用户主题状态：选中的预设 + 逐项覆盖（只存被改过的 token）。 */
export interface ThemeState {
  preset: string;
  overrides: Partial<ThemeColors>;
}

const STORAGE_KEY = "prisma-editor.theme.v1";
const HEX = /^#[0-9a-fA-F]{6}$/;

const TOKEN_KEYS = new Set<string>(
  THEME_TOKEN_GROUPS.flatMap((group) => group.tokens.map((token) => token.key)),
);

export function presetById(id: string): ThemePreset {
  return THEME_PRESETS.find((preset) => preset.id === id) ?? THEME_PRESETS[0];
}

export function normalizeHex(value: unknown, fallback: string): string {
  if (typeof value !== "string") return fallback;
  const trimmed = value.trim();
  if (!HEX.test(trimmed)) return fallback;
  return trimmed.toLowerCase();
}

export function resolveThemeColors(state: ThemeState): ThemeColors {
  return { ...presetById(state.preset).colors, ...state.overrides };
}

export function themeMode(state: ThemeState): "dark" | "light" {
  return presetById(state.preset).mode;
}

export function isOverridden(state: ThemeState, key: ThemeTokenKey): boolean {
  return Object.prototype.hasOwnProperty.call(state.overrides, key);
}

/** 把主题写成 CSS 变量；`color-scheme` 让原生滚动条/输入框跟随深浅色。 */
export function applyTheme(state: ThemeState): void {
  const colors = resolveThemeColors(state);
  const root = document.documentElement;
  for (const group of THEME_TOKEN_GROUPS) {
    for (const token of group.tokens) {
      root.style.setProperty(token.cssVar, colors[token.key]);
    }
  }
  root.style.colorScheme = themeMode(state);
  root.dataset.theme = state.preset;
}

export function loadThemeState(): ThemeState {
  const fallback: ThemeState = { preset: DEFAULT_THEME_PRESET, overrides: {} };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return fallback;
    const value = JSON.parse(raw) as Partial<ThemeState>;
    const colors = presetById(typeof value.preset === "string" ? value.preset : "").colors;
    const overrides: Partial<ThemeColors> = {};
    const rawOverrides = value.overrides;
    if (rawOverrides && typeof rawOverrides === "object") {
      for (const [key, entry] of Object.entries(rawOverrides)) {
        if (!TOKEN_KEYS.has(key)) continue;
        const token = key as ThemeTokenKey;
        const normalized = normalizeHex(entry, colors[token]);
        if (normalized !== colors[token]) overrides[token] = normalized;
      }
    }
    return {
      preset: presetById(typeof value.preset === "string" ? value.preset : "").id,
      overrides,
    };
  } catch {
    return fallback;
  }
}

export function saveThemeState(state: ThemeState): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // localStorage 不可用时仅当前会话生效。
  }
}
