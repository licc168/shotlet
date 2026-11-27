export type Dock = {
  left: number;
  top: number;
  width: number;
  height: number;
  subTop: number;
};

const MARGIN = 8;
const GAP = 10;

function clamp(value: number, min: number, max: number): number {
  if (max < min) return min;
  return Math.min(max, Math.max(min, value));
}

/// Place the capture toolbar against the selection, and keep the whole bar inside the view.
/// A wrapped or zero measurement must not be able to push it off the screen.
export function dockToolbar(
  selLeft: number,
  selTop: number,
  selWidth: number,
  selHeight: number,
  viewW: number,
  viewH: number,
  barW: number,
  barH: number,
  subH: number,
): Dock {
  const width = Math.min(Math.max(1, barW), Math.max(1, viewW - MARGIN * 2));
  const height = Math.min(Math.max(1, barH), Math.max(1, viewH - MARGIN * 2));
  const subHeight = Math.min(Math.max(0, subH), Math.max(0, viewH - MARGIN * 2));
  let left = selLeft + selWidth / 2 - width / 2;
  left = clamp(left, MARGIN, viewW - MARGIN - width);
  const below = selTop + selHeight + GAP;
  const above = selTop - GAP - height;
  let top = below;
  if (below + height > viewH - MARGIN) top = above;
  if (top < MARGIN || top + height > viewH - MARGIN) top = viewH - MARGIN - height;
  top = clamp(top, MARGIN, Math.max(MARGIN, viewH - MARGIN - height));
  let subTop = top - GAP - subHeight;
  if (subHeight > 0 && subTop < MARGIN) subTop = top + height + GAP;
  if (subHeight > 0 && subTop + subHeight > viewH - MARGIN) subTop = Math.max(MARGIN, viewH - MARGIN - subHeight);
  if (subHeight > 0) subTop = clamp(subTop, MARGIN, Math.max(MARGIN, viewH - MARGIN - subHeight));
  return { left, top, width, height, subTop };
}
