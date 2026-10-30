function normalizeExecutablePath(value) {
  if (typeof value !== "string") return null;
  let path = value.trim();
  if (!path) return null;

  if (path.startsWith('"') && path.endsWith('"') && path.length >= 2) {
    path = path.slice(1, -1).trim();
  }
  if (!path || /["\u0000-\u001f\u007f]/.test(path) || /%[^%]+%/.test(path)) return null;
  path = path.replace(/\//g, "\\");

  if (/^\\\\\?\\unc\\/i.test(path)) path = "\\\\" + path.slice(8);
  else if (path.startsWith("\\\\?\\")) path = path.slice(4);

  let segments;
  let prefix;
  if (/^[a-z]:\\/i.test(path)) {
    prefix = path.slice(0, 2).toUpperCase();
    segments = path.slice(3).split("\\").filter(Boolean);
  } else if (path.startsWith("\\\\")) {
    const uncParts = path.slice(2).split("\\").filter(Boolean);
    if (uncParts.length < 3) return null;
    prefix = `\\\\${uncParts[0]}\\${uncParts[1]}`;
    segments = uncParts.slice(2);
  } else {
    return null;
  }

  if (segments.some((segment) => segment === "." || segment === ".." || /[*?:<>|]/.test(segment))) return null;
  const normalized = `${prefix}\\${segments.join("\\")}`.replace(/\\+/g, (run) => run.length > 1 && run.startsWith("\\\\") ? "\\\\" : "\\");
  if (!/\.exe$/i.test(normalized)) return null;
  return normalized.toLowerCase();
}

function executablePathFromCommand(command) {
  if (typeof command !== "string") return null;
  const trimmed = command.trim();
  if (!trimmed) return null;

  let executable;
  if (trimmed.startsWith('"')) {
    const closingQuote = trimmed.indexOf('"', 1);
    if (closingQuote < 0) return null;
    const suffix = trimmed.slice(closingQuote + 1);
    if (suffix && !/^\s/.test(suffix)) return null;
    executable = trimmed.slice(1, closingQuote);
  } else {
    // Unquoted paths containing spaces are ambiguous under Windows command-line parsing.
    const match = /^([^\s"]+\.exe)(?=\s|$)/i.exec(trimmed);
    if (!match) return null;
    executable = match[1];
  }
  return normalizeExecutablePath(executable);
}

/**
 * Match a Windows startup-impact executable path to exactly one startup command.
 * Basename-only matching is intentionally never used.
 */
export function matchStartupImpactPathToItem(impactPath, items) {
  const target = normalizeExecutablePath(impactPath);
  if (!target || !Array.isArray(items)) return { kind: "none" };

  const matches = items.filter((item) => executablePathFromCommand(item?.cmd) === target);
  if (matches.length === 1) return { kind: "exact", item: matches[0] };
  if (matches.length > 1) return { kind: "ambiguous", items: matches };
  return { kind: "none" };
}
