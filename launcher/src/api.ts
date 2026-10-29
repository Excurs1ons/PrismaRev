import { invoke } from "@tauri-apps/api/core";

export type EditorInfo = {
  found: boolean;
  path: string | null;
  version: string;
  hint: string | null;
};

export type HubEnvironment = {
  platform: "desktop" | "mobile";
  editor: EditorInfo;
  repoRoot: string | null;
};

export type ProjectEntry = {
  path: string;
  name: string;
  exists: boolean;
  valid: boolean;
  sceneCount: number;
  hasGameCrate: boolean;
  lastOpened: number | null;
  addedAt: number;
  running: boolean;
};

export const hub = {
  environment: () => invoke<HubEnvironment>("hub_environment"),
  editorInfo: () => invoke<EditorInfo>("hub_editor_info"),
  projects: () => invoke<ProjectEntry[]>("hub_projects"),
  addProject: (path: string) => invoke<ProjectEntry>("hub_add_project", { path }),
  removeProject: (path: string) => invoke<ProjectEntry[]>("hub_remove_project", { path }),
  createProject: (name: string, location: string) =>
    invoke<ProjectEntry>("hub_create_project", { name, location }),
  openProject: (path: string) => invoke<ProjectEntry>("hub_open_project", { path }),
  revealProject: (path: string) => invoke<void>("hub_reveal_project", { path }),
  launchGame: (config: string) => invoke<void>("launch_game", { config }),
};

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
