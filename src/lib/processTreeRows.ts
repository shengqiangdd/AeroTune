type ProcessTreeNode = {
  pid: number;
  parent_pid?: number | null;
};

export type ProcessTreeRow<T extends ProcessTreeNode> = {
  proc: T;
  depth: number;
};

/** Builds ordered depth-first rows for the current filtered process snapshot. */
export function buildProcessTreeRows<T extends ProcessTreeNode>(
  filtered: readonly T[],
): ProcessTreeRow<T>[] {
  const byPid = new Map(filtered.map((proc) => [proc.pid, proc] as const));
  const children = new Map<number, T[]>();
  const roots: T[] = [];
  for (const proc of filtered) {
    const parent = proc.parent_pid == null ? null : byPid.get(proc.parent_pid);
    if (!parent || parent.pid === proc.pid) roots.push(proc);
    else {
      const siblings = children.get(parent.pid);
      if (siblings) siblings.push(proc);
      else children.set(parent.pid, [proc]);
    }
  }

  const rows: ProcessTreeRow<T>[] = [];
  const seen = new Set<number>();
  const visit = (proc: T, depth: number) => {
    if (seen.has(proc.pid)) return;
    seen.add(proc.pid);
    rows.push({ proc, depth });
    for (const child of children.get(proc.pid) ?? []) visit(child, depth + 1);
  };
  roots.forEach((root) => visit(root, 0));
  filtered.forEach((proc) => {
    if (!seen.has(proc.pid)) visit(proc, 0);
  });
  return rows;
}
