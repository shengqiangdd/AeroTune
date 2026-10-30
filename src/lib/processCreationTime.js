const WINDOWS_UNIX_EPOCH_FILETIME_TICKS = 116_444_736_000_000_000n;
const FILETIME_TICKS_PER_MILLISECOND = 10_000n;
const FILETIME_TOKEN_PATTERN = /^[0-9a-f]{16}$/i;

/**
 * Formats the exact FILETIME creation token as a UTC timestamp with 100-ns precision.
 * Returns null when the token is absent, malformed, or predates the Unix epoch.
 * @param {string | undefined} token
 * @returns {string | null}
 */
export function formatWindowsFiletimeUtc(token) {
  if (typeof token !== "string" || !FILETIME_TOKEN_PATTERN.test(token)) return null;

  let filetimeTicks;
  try {
    filetimeTicks = BigInt(`0x${token}`);
  } catch {
    return null;
  }

  const unixTicks = filetimeTicks - WINDOWS_UNIX_EPOCH_FILETIME_TICKS;
  if (unixTicks < 0n) return null;

  const unixMilliseconds = unixTicks / FILETIME_TICKS_PER_MILLISECOND;
  const date = new Date(Number(unixMilliseconds));
  if (!Number.isFinite(date.getTime())) return null;

  const subMillisecondTicks = String(unixTicks % FILETIME_TICKS_PER_MILLISECOND).padStart(4, "0");
  return date
    .toISOString()
    .replace("T", " ")
    .replace(/\.(\d{3})Z$/, (_, milliseconds) => `.${milliseconds}${subMillisecondTicks} UTC`);
}
