/** 显示与颜色转换的小工具。 */

/** 数值显示：整数原样，浮点保留约 7 位有效数字。 */
export function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (Number.isInteger(value)) return String(value);
  return String(Number(value.toPrecision(7)));
}

/** 字节数显示。 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/** 归一化 RGB（0..1）→ `#rrggbb`。 */
export function floatsToHex(rgb: number[]): string {
  const channel = (value: number) =>
    Math.round(clamp01(value ?? 0) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${channel(rgb[0])}${channel(rgb[1])}${channel(rgb[2])}`;
}

/** `#rrggbb` → 归一化 RGB（0..1）。 */
export function hexToFloats(hex: string): number[] {
  const clean = hex.replace("#", "");
  return [
    parseInt(clean.slice(0, 2), 16) / 255,
    parseInt(clean.slice(2, 4), 16) / 255,
    parseInt(clean.slice(4, 6), 16) / 255,
  ];
}

const COLOR_NAME = /(color|background|tint|ambient|albedo|emissive|clear)/i;

/**
 * 是否是「颜色」字段：名字像颜色 + 3/4 个数值组成的数组。
 *
 * 引擎里颜色统一是归一化 `[r,g,b(,a)]`（`glam::Vec3` 或 `[f32;4]`），
 * 所以按值形状 + 字段名双重判断，避免把 `euler_xyz` 这类 Vec3 误当颜色。
 */
export function isColorField(fieldName: string | undefined, value: unknown): value is number[] {
  if (!Array.isArray(value)) return false;
  if (value.length !== 3 && value.length !== 4) return false;
  if (!value.every((item) => typeof item === "number" && Number.isFinite(item))) return false;
  return !!fieldName && COLOR_NAME.test(fieldName);
}
