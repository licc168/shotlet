export type StrokePoint = { x: number; y: number };

const STROKE_MIN_STEP = 1.25;

export function appendStrokePoint<T extends { points: StrokePoint[] }>(stroke: T, point: StrokePoint, minStep = STROKE_MIN_STEP): T {
  const last = stroke.points[stroke.points.length - 1];
  if (last) {
    const dx = point.x - last.x;
    const dy = point.y - last.y;
    if (dx * dx + dy * dy < minStep * minStep) return stroke;
  }
  return { ...stroke, points: [...stroke.points, point] };
}
