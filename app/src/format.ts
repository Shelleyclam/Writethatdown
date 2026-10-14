/** "Today", "Yesterday", or "Monday, 12 Oct". */
export function dayLabel(ms: number): string {
  const date = new Date(ms);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return "Today";
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  const sameYear = date.getFullYear() === today.getFullYear();
  return date.toLocaleDateString(undefined, {
    weekday: "long",
    day: "numeric",
    month: "short",
    year: sameYear ? undefined : "numeric",
  });
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** "12 Oct 2026" */
export function formatDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** "chrome.exe" -> "chrome" */
export function appName(exe: string): string {
  return exe.replace(/\.exe$/i, "");
}

/** Group items (newest first) under day labels, keeping their order. */
export function groupByDay<T>(items: T[], time: (item: T) => number): [string, T[]][] {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const day = dayLabel(time(item));
    groups.set(day, [...(groups.get(day) ?? []), item]);
  }
  return [...groups];
}

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
