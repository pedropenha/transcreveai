export type DiffPart = {
  kind: "equal" | "added" | "removed";
  text: string;
};

export function moveHistorySelection(
  ids: number[],
  selectedId: number | null,
  direction: "next" | "previous",
): number | null {
  if (ids.length === 0) return null;
  const current = selectedId === null ? -1 : ids.indexOf(selectedId);
  if (direction === "next") return ids[Math.min(current + 1, ids.length - 1)];
  if (current <= 0) return ids[0];
  return ids[current - 1];
}

export function buildWordDiff(before: string, after: string): DiffPart[] {
  const left = before.trim().split(/\s+/).filter(Boolean);
  const right = after.trim().split(/\s+/).filter(Boolean);
  const rows = left.length + 1;
  const columns = right.length + 1;
  if (rows * columns > 1_000_000) {
    return [
      ...(before.trim()
        ? [{ kind: "removed" as const, text: before.trim() }]
        : []),
      ...(after.trim() ? [{ kind: "added" as const, text: after.trim() }] : []),
    ];
  }
  const lcs = new Uint32Array(rows * columns);

  for (let i = left.length - 1; i >= 0; i -= 1) {
    for (let j = right.length - 1; j >= 0; j -= 1) {
      lcs[i * columns + j] =
        left[i] === right[j]
          ? lcs[(i + 1) * columns + j + 1] + 1
          : Math.max(lcs[(i + 1) * columns + j], lcs[i * columns + j + 1]);
    }
  }

  const parts: DiffPart[] = [];
  let i = 0;
  let j = 0;
  while (i < left.length || j < right.length) {
    if (i < left.length && j < right.length && left[i] === right[j]) {
      parts.push({ kind: "equal", text: left[i] });
      i += 1;
      j += 1;
    } else if (
      j < right.length &&
      (i === left.length ||
        lcs[i * columns + j + 1] >= lcs[(i + 1) * columns + j])
    ) {
      parts.push({ kind: "added", text: right[j] });
      j += 1;
    } else {
      parts.push({ kind: "removed", text: left[i] });
      i += 1;
    }
  }
  return parts;
}
