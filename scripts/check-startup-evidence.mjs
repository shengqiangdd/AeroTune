import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { buildStartupEvidenceByItem, startupEvidenceExportLabel } from "../src/lib/startupEvidence.js";
import { startupTrustLabel } from "../src/lib/startupTrust.js";

const item = (id, name, cmd) => ({ id, name, cmd, source: "Run", enabled: true });
const steam = item("steam", "Steam", '"C:\\Program Files (x86)\\Steam\\steam.exe" -silent');
const discord = item("discord", "Discord", '"C:\\Users\\Alice\\AppData\\Local\\Discord\\Update.exe" --processStart Discord.exe');
const measured = {
  ts: 1_800_000_000,
  name: "A different display name",
  path: "c:/program files (x86)/steam/steam.EXE",
  total_ms: 1200,
  degradation_ms: 320,
};

const evidence = buildStartupEvidenceByItem([steam, discord], [measured]);
assert.equal(evidence.get("steam")?.measuredImpacts.length, 1, "Event 101 is linked only by unique complete executable path");
assert.equal(evidence.get("steam")?.heuristic, "游戏平台", "name heuristics remain separate from measured impact");
assert.equal(evidence.get("discord")?.measuredImpacts.length, 0, "similar application names do not inherit another item's measurement");
assert.equal(evidence.get("discord")?.heuristic, "即时通讯");
assert.equal(
  buildStartupEvidenceByItem([steam], [{ ...measured, path: "\\\\?\\C:\\Program Files (x86)\\Steam\\steam.exe" }]).get("steam")?.measuredImpacts.length,
  1,
  "extended local-drive paths normalize to the same executable",
);
assert.match(startupEvidenceExportLabel(evidence.get("steam")), /Windows 事件 101 实测影响 320 ms/);
assert.match(startupEvidenceExportLabel(evidence.get("steam")), /名称启发式（非实测）：游戏平台/);
assert.equal(startupEvidenceExportLabel(evidence.get("unknown")), "暂无可关联记录（不代表没有影响）");
assert.equal(startupTrustLabel("signed", "Contoso Ltd."), "内嵌签名有效 · Contoso Ltd.");
assert.equal(startupTrustLabel("signed", null), "内嵌签名有效 · 未读取到证书显示名");
assert.equal(startupTrustLabel("unsigned", null), "无内嵌签名 · 目录签名未检查");
assert.equal(startupTrustLabel("unknown", null), "未验证 · 不代表安全");

const duplicateA = item("duplicate-a", "Widget A", '"C:\\Vendor\\Widget.exe"');
const duplicateB = item("duplicate-b", "Widget B", '"c:/vendor/widget.exe" --other');
const ambiguous = buildStartupEvidenceByItem([duplicateA, duplicateB], [{ ...measured, path: "C:\\Vendor\\Widget.exe" }]);
assert.equal(ambiguous.get("duplicate-a")?.measuredImpacts.length, 0, "ambiguous duplicate paths are never arbitrarily assigned");
assert.equal(ambiguous.get("duplicate-b")?.measuredImpacts.length, 0);

for (const path of [
  "steam.exe",
  "%ProgramFiles%\\Steam\\steam.exe",
  "\\\\server\\share\\Steam.exe",
]) {
  const unverified = buildStartupEvidenceByItem([steam], [{ ...measured, path }]);
  assert.equal(unverified.get("steam")?.measuredImpacts.length, 0, `${path} is not guessed as an exact measurement`);
}
const unquotedCommand = item("unquoted", "Steam", "C:\\Program Files (x86)\\Steam\\steam.exe -silent");
assert.equal(buildStartupEvidenceByItem([unquotedCommand], [measured]).get("unquoted")?.measuredImpacts.length, 0, "unquoted command paths with spaces remain ambiguous");
const uncItem = item("unc", "Steam", '"\\\\server\\share\\Steam.exe"');
assert.equal(buildStartupEvidenceByItem([uncItem], [{ ...measured, path: "\\\\server\\share\\Steam.exe" }]).get("unc")?.measuredImpacts.length, 0, "UNC evidence stays unverified even when the text matches exactly");
const shortcut = item("shortcut", "Steam", '"C:\\Users\\Alice\\Start Menu\\Steam.lnk"');
assert.equal(buildStartupEvidenceByItem([shortcut], [measured]).get("shortcut")?.measuredImpacts.length, 0, "shortcut targets are not inferred from the shortcut path");
assert.equal(buildStartupEvidenceByItem([steam], [{ ...measured, degradation_ms: Number.NaN }]).get("steam")?.measuredImpacts.length, 0, "invalid impact values are excluded");

const startup = readFileSync("src/pages/Startup.tsx", "utf8");
const backend = readFileSync("src-tauri/src/startup.rs", "utf8");
const profile = readFileSync("src-tauri/src/profile.rs", "utf8");
const api = readFileSync("src/lib/api.ts", "utf8");
const releaseReadiness = readFileSync("docs/RELEASE_READINESS.md", "utf8");
assert.match(releaseReadiness, /startup-item table now discloses its covered\/excluded sources and shows bounded embedded-Authenticode status/);
assert.match(backend, /pub async fn list_startup\(\)[\s\S]*spawn_blocking[\s\S]*attach_startup_signature_context[\s\S]*map_err/);
assert.match(backend, /fn startup_signature_candidate[\s\S]*parse_delay_command\(cmd\)\.ok\(\)\?[\s\S]*Component::ParentDir/);
assert.match(backend, /fn is_safe_local_signature_target[\s\S]*symlink_metadata[\s\S]*FILE_ATTRIBUTE_REPARSE_POINT[\s\S]*metadata\.is_file\(\)/);
assert.match(backend, /STARTUP_SIGNATURE_BUDGET_PER_LIST: usize = 8[\s\S]*STARTUP_SIGNATURE_CANDIDATE_CAP_PER_LIST: usize = 32/);
assert.match(backend, /signature_status: "unknown"[\s\S]*signer_subject: None/);
assert.match(profile, /GetProcAddress[\s\S]*WTHelperProvDataFromStateData[\s\S]*WTHelperGetProvSignerFromChain[\s\S]*WTHelperGetProvCertFromChain/);
assert.match(profile, /status == 0[\s\S]*signer_subject_from_state[\s\S]*WTD_STATEACTION_CLOSE/);
assert.match(profile, /SIGNATURE_CACHE_TTL[\s\S]*verify_signature_evidence_cached[\s\S]*status != SignatureStatus::Unknown/);
assert.match(api, /signature_status: "signed" \| "unsigned" \| "unknown"[\s\S]*signer_subject: string \| null/);
assert.match(startup, /startupTrustLabel\(it\.signature_status, it\.signer_subject\)/);
assert.match(startup, /startupTrustLabel\(item\.signature_status, item\.signer_subject\)/);
assert.match(startup, /const requestId = \+\+startupRequestId\.current[\s\S]*requestId !== startupRequestId\.current[\s\S]*setStartupStatus\("error"\)[\s\S]*setStartupLoadError/);
assert.match(startup, /startupStatus === "loading"[\s\S]*上次快照[\s\S]*startupStatus === "error"[\s\S]*role="alert"[\s\S]*重试读取/);
assert.match(startup, /disabled=\{startupStatus !== "ready" \|\| loading\}/);
assert.match(startup, /Toggle label=\{`启动项：\$\{it\.name\}`\}[\s\S]*disabled=\{startupStatus !== "ready" \|\| loading\}/);
assert.match(startup, /onClick=\{exportStartupCsv\} disabled=\{exportingStartup \|\| loading \|\| startupStatus !== "ready"[\s\S]*导出 CSV/);
assert.match(startup, /本表仅覆盖当前注册表视图的 HKCU\/HKLM Run[\s\S]*启动文件夹中的 \.lnk\/\.exe\/\.bat 项[\s\S]*计划任务在下方单列[\s\S]*不在本表内/);
assert.match(startup, /每次最多筛选 32 个候选[\s\S]*对 8 个新目标执行 WinVerifyTrust[\s\S]*超出限额均显示“未验证”[\s\S]*“刷新”重试[\s\S]*目录签名可能显示“无内嵌签名”[\s\S]*未知也不代表安全/);
assert.match(startup, /buildStartupEvidenceByItem\(items, impacts \?\? \[\]\)/);
assert.match(startup, /startupEvidenceById\.get\(it\.id\)/);
assert.match(startup, /startupEvidenceExportLabel\(startupEvidenceById\.get\(item\.id\)\)/);
assert.match(startup, /Windows 事件 101 实测/);
assert.match(startup, /名称启发式（非实测）/);
assert.match(startup, /完整路径唯一匹配/);
assert.match(startup, /没有事件记录不代表没有影响/);
assert.match(startup, /证据提示/);
assert.doesNotMatch(startup, /建议延迟 ·/, "name-only hints must not appear as a delay recommendation");

console.log("Startup trust/evidence contract passed: bounded signer context, source scope, unknown handling, exact impact matches, and cautious copy");
