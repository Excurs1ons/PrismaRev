/** Editor-only preferences (per machine, stored in localStorage). */
export interface EditorPreferences {
  autoSaveSeconds: number;
  confirmDelete: boolean;
  compactMode: boolean;
  maxLogLines: number;
  autoOpenLog: boolean;
  defaultPlayScene: string;
}

export const DEFAULT_PREFERENCES: EditorPreferences = {
  autoSaveSeconds: 0,
  confirmDelete: false,
  compactMode: false,
  maxLogLines: 5000,
  autoOpenLog: true,
  defaultPlayScene: "",
};

const STORAGE_KEY = "prisma-editor.prefs.v1";

function clampInt(value: unknown, min: number, max: number, fallback: number): number {
  const number = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(number)) return fallback;
  return Math.min(max, Math.max(min, Math.round(number)));
}

export function loadPreferences(): EditorPreferences {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_PREFERENCES };
    const value = JSON.parse(raw) as Partial<EditorPreferences>;
    return {
      autoSaveSeconds: clampInt(value.autoSaveSeconds, 0, 600, DEFAULT_PREFERENCES.autoSaveSeconds),
      confirmDelete:
        typeof value.confirmDelete === "boolean"
          ? value.confirmDelete
          : DEFAULT_PREFERENCES.confirmDelete,
      compactMode:
        typeof value.compactMode === "boolean"
          ? value.compactMode
          : DEFAULT_PREFERENCES.compactMode,
      maxLogLines: clampInt(value.maxLogLines, 200, 20000, DEFAULT_PREFERENCES.maxLogLines),
      autoOpenLog:
        typeof value.autoOpenLog === "boolean"
          ? value.autoOpenLog
          : DEFAULT_PREFERENCES.autoOpenLog,
      defaultPlayScene:
        typeof value.defaultPlayScene === "string"
          ? value.defaultPlayScene
          : DEFAULT_PREFERENCES.defaultPlayScene,
    };
  } catch {
    return { ...DEFAULT_PREFERENCES };
  }
}

export function savePreferences(preferences: EditorPreferences): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // localStorage 不可用时仅当前会话生效。
  }
}
