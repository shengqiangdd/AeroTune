import assert from "node:assert/strict";
import {
  buildChangeJournalCsv,
  buildChangeJournalJson,
  filterChanges,
  getChangeCategory,
  getChangeRevertTarget,
  summarizeUndoTargets,
} from "../src/lib/changeJournal.js";

const changes = [
  { id: 1, ts: 1_750_000_000, desc: "DNS preset changed", group_label: "network", revert: { t: "dns", snapshot: { oldValue: "secret-value" } } },
  { id: 2, ts: 1_750_000_100, desc: "Read-only observation", revert: { t: "none" } },
  { id: 3, ts: 1_750_000_200, desc: '=HYPERLINK("https://example.invalid","open")', revert: { t: "registry", snapshot: { oldValue: "private-registry-value" } } },
];

assert.equal(filterChanges(changes, "network").length, 1);
assert.equal(filterChanges(changes, "dns", "dns").length, 1);
assert.deepEqual(filterChanges(changes, "", "all", "reversible").map((change) => change.id), [1, 3]);
assert.deepEqual(filterChanges(changes, "", "other").map((change) => change.id), [2]);
assert.equal(getChangeCategory("power_plan"), "电源");
const forgedTarget = {
  id: 4,
  desc: "misleading benign label",
  revert: { t: "registry", key: `HKLM\\SOFTWARE\\Microsoft\\Windows\\Run${String.fromCharCode(10)}Fake`, value: "Start" },
};
assert.equal(getChangeRevertTarget(forgedTarget), "注册表 HKLM\\SOFTWARE\\Microsoft\\Windows\\Run Fake\\Start → 原值未知");
assert.deepEqual(summarizeUndoTargets([changes[0], forgedTarget], 1), {
  complete: false,
  tooMany: true,
  tooLong: false,
  invalidTarget: true,
  total: 2,
  visible: ["1. DNS 网卡 未知 → 原设置未知"],
});
const authorizedDwordTarget = {
  revert: {
    t: "registry",
    key: "HKLM\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
    value: "HwSchMode",
    old: { k: "raw", vtype: 4, bytes: [2, 0, 0, 0] },
  },
};
assert.equal(getChangeRevertTarget(authorizedDwordTarget), "注册表 HKLM\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers\\HwSchMode → DWORD 2 (0x00000002)");
assert.equal(summarizeUndoTargets([authorizedDwordTarget]).complete, true);
const nicPnpTarget = {
  revert: {
    t: "registry",
    key: "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\0000",
    value: "PnPCapabilities",
    old: { k: "dword", value: 0 },
    nic_guid: "{01234567-89AB-CDEF-0123-456789ABCDEF}",
  },
};
assert.ok(getChangeRevertTarget(nicPnpTarget).includes("适配器 GUID {01234567-89AB-CDEF-0123-456789ABCDEF}"));
assert.equal(summarizeUndoTargets([nicPnpTarget]).complete, true);
const legacyNicPnpTarget = { revert: { ...nicPnpTarget.revert, nic_guid: undefined } };
assert.equal(summarizeUndoTargets([legacyNicPnpTarget]).invalidTarget, true);
const fullHiveNicTarget = {
  revert: {
    ...nicPnpTarget.revert,
    key: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\0000",
  },
};
assert.ok(getChangeRevertTarget(fullHiveNicTarget).startsWith("注册表 HKEY_LOCAL_MACHINE\\"));
assert.equal(summarizeUndoTargets([fullHiveNicTarget]).complete, true);
assert.equal(summarizeUndoTargets([{ revert: { ...fullHiveNicTarget.revert, nic_guid: undefined } }]).invalidTarget, true);
assert.equal(summarizeUndoTargets([{ revert: { ...nicPnpTarget.revert, nic_guid: "spoof" } }]).invalidTarget, true);
const dnsTarget = { revert: { t: "dns", iface: "Ethernet", old: { ipv4: ["1.1.1.1"], ipv6: null } } };
assert.equal(getChangeRevertTarget(dnsTarget), "DNS 网卡 Ethernet → IPv4 1.1.1.1; IPv6 自动获取");
assert.equal(summarizeUndoTargets([dnsTarget]).complete, true);
const longScopedIpv6 = `ffff:ffff:ffff:ffff:ffff:ffff:255.255.255.255%${"a".repeat(32)}`;
const dnsTargetAtBackendBounds = {
  revert: {
    t: "dns",
    iface: "网卡".repeat(128),
    old: { ipv4: Array(16).fill("255.255.255.255"), ipv6: [longScopedIpv6] },
  },
};
assert.equal(Array.from(longScopedIpv6).length, 78);
assert.equal(summarizeUndoTargets([dnsTargetAtBackendBounds]).complete, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "dns", iface: "x", old: { ipv4: Array(17).fill("1.1.1.1"), ipv6: null } } }]).invalidTarget, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "dns", iface: "x".repeat(257), old: { ipv4: null, ipv6: null } } }]).invalidTarget, true);
const verboseDnsTarget = {
  revert: {
    t: "dns",
    iface: "Ethernet",
    old: { ipv4: Array(16).fill("255.255.255.255"), ipv6: Array(16).fill(longScopedIpv6) },
  },
};
assert.equal(summarizeUndoTargets(Array(12).fill(verboseDnsTarget)).tooLong, true);
assert.equal(getChangeRevertTarget({ revert: { t: "epp", old: 45 } }), "系统能源性能偏好（EPP）恢复为 45");
assert.equal(summarizeUndoTargets([{ revert: { t: "epp", old: 101 } }]).invalidTarget, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "power_plan", guid: "381b4222-f694-41f0-9685-ff5bb260df2e" } }]).complete, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "power_plan", guid: "scheme_min" } }]).invalidTarget, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "tcp", level: "normal" } }]).complete, true);
assert.equal(summarizeUndoTargets([{ revert: { t: "tcp", level: "normal; evil" } }]).invalidTarget, true);
assert.equal(getChangeRevertTarget({ revert: { t: "dns", iface: `Ethernet${String.fromCharCode(10)}spoof` } }), "DNS 网卡 Ethernet spoof → 原设置未知");
const bidiTarget = { revert: { t: "dns", iface: `Ethernet${String.fromCharCode(0x202e)}spoof` } };
assert.equal(getChangeRevertTarget(bidiTarget), "DNS 网卡 Ethernet spoof → 原设置未知");
assert.equal(summarizeUndoTargets([bidiTarget]).invalidTarget, true);
const overlongTarget = { revert: { t: "registry", key: `HKLM\\Software\\${"A".repeat(600)}`, value: "Start" } };
assert.equal(summarizeUndoTargets([overlongTarget]).invalidTarget, true);
assert.ok(getChangeRevertTarget(overlongTarget).includes("…"));

const csv = buildChangeJournalCsv(changes, "1.5.3");
assert.ok(csv.startsWith("\uFEFF"));
assert.ok(csv.includes("'=HYPERLINK"), "formula-leading descriptions are neutralized");
assert.ok(csv.includes("\r\n"));

const report = JSON.parse(buildChangeJournalJson(changes, "1.5.3"));
assert.equal(report.count, changes.length);
assert.equal(report.app_version, "1.5.3");
assert.equal(report.changes[0].reversible, true);
assert.equal("revert" in report.changes[0], false, "JSON export must not include rollback snapshots");
assert.ok(!JSON.stringify(report).includes("private-registry-value"));

console.log("PASS journal filtering, category/reversibility filters, formula-safe CSV, and snapshot-free JSON export");
