import type { Track } from "./api";

export interface HistoryEntry {
  id: number;
  played_at: number;
  track: Track;
}

/** Local calendar time, deliberately independent of the machine's locale. */
export function historyDate(epoch: number): string {
  const d = new Date(epoch * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getDate())}-${pad(d.getMonth() + 1)}-${pad(d.getFullYear() % 100)}, ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function historyAgo(epoch: number, current = Date.now()): string {
  const seconds = Math.max(0, Math.floor(current / 1000 - epoch));
  if (seconds < 60) return "just now";
  const units: [number, string][] = [
    [365 * 86400, "year"], [30 * 86400, "month"], [7 * 86400, "week"],
    [86400, "day"], [3600, "hour"], [60, "minute"],
  ];
  for (const [size, unit] of units) {
    if (seconds >= size) {
      const n = Math.floor(seconds / size);
      return `${n} ${unit}${n === 1 ? "" : "s"} ago`;
    }
  }
  return "just now";
}
