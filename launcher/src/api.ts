import { invoke } from "@tauri-apps/api/core";

/** 拉起游戏（Android 走 Kotlin plugin 启动 GameActivity；桌面走本地二进制）。 */
export const launcher = {
  launchGame: (config: string) => invoke<void>("launch_game", { config }),
};

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
