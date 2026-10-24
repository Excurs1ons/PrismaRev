/** Clamp a fixed-position context menu so it stays inside the viewport. */
export function clampMenuPosition(
  x: number,
  y: number,
  width = 190,
  height = 170,
): { x: number; y: number } {
  return {
    x: Math.max(4, Math.min(x, window.innerWidth - width - 4)),
    y: Math.max(4, Math.min(y, window.innerHeight - height - 4)),
  };
}
