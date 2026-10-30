import assert from "node:assert/strict";
import { formatWindowsFiletimeUtc } from "../src/lib/processCreationTime.js";

const WINDOWS_UNIX_EPOCH_FILETIME_TICKS = 116_444_736_000_000_000n;
const tokenFor = (filetimeTicks) => filetimeTicks.toString(16).padStart(16, "0");

assert.equal(
  formatWindowsFiletimeUtc(tokenFor(WINDOWS_UNIX_EPOCH_FILETIME_TICKS)),
  "1970-01-01 00:00:00.0000000 UTC",
  "the Windows FILETIME epoch must map to the Unix epoch",
);
assert.equal(
  formatWindowsFiletimeUtc(tokenFor(WINDOWS_UNIX_EPOCH_FILETIME_TICKS + 12_345_678n)),
  "1970-01-01 00:00:01.2345678 UTC",
  "the visible timestamp must preserve all seven fractional digits",
);
assert.equal(
  formatWindowsFiletimeUtc(tokenFor(WINDOWS_UNIX_EPOCH_FILETIME_TICKS + 12_345_678n).toUpperCase()),
  "1970-01-01 00:00:01.2345678 UTC",
  "uppercase lossless tokens must format identically",
);

for (const invalid of [undefined, "", "000000000000000", "00000000000000000", "000000000000000g", "0000000000000000"]) {
  assert.equal(formatWindowsFiletimeUtc(invalid), null, `invalid token should not format: ${String(invalid)}`);
}
assert.match(
  formatWindowsFiletimeUtc("ffffffffffffffff") ?? "",
  /^\+060056-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{7} UTC$/,
  "the entire 64-bit FILETIME range must remain representable without losing 100-ns digits",
);

console.log("Process creation-time formatter passed: strict token validation and exact 100-ns UTC display");
