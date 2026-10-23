import type { InjectionKey, Ref } from "vue";

/**
 * 跨层共享的注入键（避免把 `assets` 逐层当 prop 传进递归的字段编辑器）。
 *
 * - `ASSETS_KEY`：`assets/` 下的候选路径（相对项目根）。
 */
export const ASSETS_KEY: InjectionKey<Ref<string[]>> = Symbol("prism-editor-assets");
