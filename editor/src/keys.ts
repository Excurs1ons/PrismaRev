import type { InjectionKey, Ref } from "vue";
import type { AssetInfo } from "./types";

/**
 * 跨层共享的注入键（避免把资产列表逐层当 prop 传进递归的字段编辑器）。
 *
 * - `ASSETS_KEY`：`assets/` 下的资产元信息。
 * - `ASSET_PICKER_KEY`：打开「资产管理器」选一个路径，返回所选值（取消为 `null`）。
 */
export const ASSETS_KEY: InjectionKey<Ref<AssetInfo[]>> = Symbol("prism-editor-assets");

export const ASSET_PICKER_KEY: InjectionKey<(current: string) => Promise<string | null>> = Symbol(
  "prism-editor-asset-picker",
);
