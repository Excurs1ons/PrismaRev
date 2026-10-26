/**
 * Fixed-row-height windowing helper.
 *
 * Panels such as the log view can hold tens of thousands of rows. Rendering
 * one DOM node per row makes scrolling and memory usage degrade long before
 * the user notices anything useful. Instead we render only the rows that are
 * (roughly) inside the scroll viewport and reserve the rest of the height with
 * a spacer, so the scrollbar keeps reflecting the real row count.
 *
 * This only works for rows with a **fixed height** — callers must keep the row
 * height in CSS and in the constant they pass here in sync.
 */

export interface WindowRange {
  /** Index of the first row that should be rendered. */
  start: number;
  /** Exclusive index of the last row that should be rendered. */
  end: number;
  /** Pixel offset of `start`, i.e. where the rendered window is translated to. */
  offset: number;
  /** Full scroll height so the container can reserve the missing rows. */
  total: number;
}

/**
 * Compute the visible row window for a fixed-height list.
 *
 * @param count          total number of rows
 * @param rowHeight      height of one row in CSS pixels (> 0)
 * @param scrollTop      current `scrollTop` of the scroll container
 * @param viewportHeight visible height of the scroll container (0 before the
 *                       first measurement; one row is rendered in that case)
 * @param overscan       extra rows above and below the viewport, which hides
 *                       the gap while scrolling
 */
export function windowRange(
  count: number,
  rowHeight: number,
  scrollTop: number,
  viewportHeight: number,
  overscan = 6,
): WindowRange {
  if (count <= 0 || rowHeight <= 0) {
    return { start: 0, end: 0, offset: 0, total: 0 };
  }

  const visible = Math.max(1, Math.ceil(viewportHeight / rowHeight));
  const first = Math.min(count - 1, Math.max(0, Math.floor(scrollTop / rowHeight)));
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, start + visible + overscan * 2);

  return { start, end, offset: start * rowHeight, total: count * rowHeight };
}
