import fs from "node:fs";

const network = fs.readFileSync("src-tauri/src/network.rs", "utf8");
const networkUi = fs.readFileSync("src/pages/Network.tsx", "utf8");
const nicLatencyUi = networkUi.slice(networkUi.indexOf("function NicLatencyCards()"));
const journal = fs.readFileSync("src-tauri/src/journal.rs", "utf8");
const setDns = network.slice(network.indexOf("pub fn set_dns("), network.indexOf("pub fn reset_dns("));
const resetDns = network.slice(network.indexOf("pub fn reset_dns("), network.indexOf("pub fn flush_dns("));
const doRevert = journal.slice(journal.indexOf("fn do_revert("));
const checks = [
  {
    ok: /fn normalize_ipv4_server[\s\S]*parse::<std::net::Ipv4Addr>/.test(network)
      && /fn parse_dns_servers[\s\S]*normalize_ipv4_server\(primary\)[\s\S]*normalize_ipv4_server\(secondary\)/.test(network)
      && /fn powershell_single_quote[\s\S]*value\.replace/.test(network),
    description: "DNS writes validate IPv4 inputs and quote interface aliases",
  },
  {
    ok: /read_dns_settings\(&iface\)[\s\S]*set_dns_v4_raw\(&iface, &servers\)[\s\S]*Revert::Dns/.test(setDns)
      && /read_dns_settings\(&iface\)[\s\S]*DnsSettings::default\(\)[\s\S]*Revert::Dns/.test(resetDns),
    description: "successful DNS set and reset operations record the original and applied states",
  },
  {
    ok: /pub struct DnsSettings[\s\S]*ipv4: Option<Vec<String>>[\s\S]*ipv6: Option<Vec<String>>/.test(journal)
      && /Tcpip\\Parameters\\Interfaces[\s\S]*Tcpip6\\Parameters\\Interfaces[\s\S]*NameServer/.test(network),
    description: "snapshots preserve automatic/static state separately for IPv4 and IPv6",
  },
  {
    ok: /fn restore_dns_exact[\s\S]*if &current != applied[\s\S]*保留恢复记录/.test(journal)
      && /match write\(iface, old\)[\s\S]*write\(iface, applied\)[\s\S]*已重新应用并验证恢复前设置[\s\S]*自动恢复当前设置也失败/.test(journal)
      && /Revert::Dns \{[\s\S]*?iface,[\s\S]*?old,[\s\S]*?applied,[\s\S]*?\} => restore_dns_exact\(/.test(doRevert),
    description: "undo compares current DNS state and compensates partial restore failures",
  },
  {
    ok: /dns_input_accepts_ipv4_and_rejects_shell_syntax/.test(network)
      && /dns_restore_commands_reset_then_reapply_each_static_family/.test(network)
      && /dns_restore_preserves_record_when_external_state_differs/.test(journal)
      && /dns_restore_compensates_known_partial_state_and_preserves_the_record/.test(journal)
      && /dns_restore_does_not_overwrite_unexpected_concurrent_changes/.test(journal)
      && /dns_restore_reports_failed_compensation_without_dropping_the_record/.test(journal),
    description: "unit coverage includes validation, family restoration, conflicts, and compensation",
  },
  {
    ok: /const runDnsAction = async[\s\S]*if \(dnsActionPending\.current \|\| tcpPending\.current\) return;[\s\S]*if \(loadPending\.current\)[\s\S]*dnsActionPending\.current = true[\s\S]*await action\(\)[\s\S]*finally[\s\S]*dnsActionPending\.current = false/.test(networkUi)
      && /disabled=\{!sel \|\| loading \|\| dnsAction !== null \|\| tcpBusy\}/.test(networkUi)
      && /disabled=\{loading \|\| dnsAction !== null \|\| tcpBusy\} onClick=\{flush\}/.test(networkUi)
      && /role="status" aria-live="polite"[\s\S]*正在应用 DNS 配置/.test(networkUi),
    description: "DNS mutations are single-flight, lock the interface, and announce progress",
  },
  {
    ok: /fn parse_net_quality\(raw: Result<String, String>\) -> Result<NetQuality, String>[\s\S]*let raw = raw\?;/.test(network)
      && /pub fn ping_quality\(\) -> Result<NetQuality, String>[\s\S]*run_ps_with_timeout_result[\s\S]*parse_net_quality/.test(network)
      && !/parse_net_quality\(crate::util::run_ps_with_timeout_result[\s\S]*?unwrap_or\(NetQuality/.test(network),
    description: "network probe command propagates execution and parse errors",
  },
  {
    ok: /ping_probe_errors_do_not_become_packet_loss_measurements[\s\S]*PowerShell 查询超时[\s\S]*malformed JSON[\s\S]*存在不可达目标/.test(network),
    description: "timeout and malformed output differ from valid all-unreachable measurements",
  },
  {
    ok: /const \[qualityError, setQualityError\]/.test(networkUi)
      && /role="alert"[\s\S]*探测未完成[\s\S]*未形成有效的丢包样本/.test(networkUi)
      && /setQualityError\(null\)/.test(networkUi),
    description: "network UI announces probe failure and clears it before retry",
  },
  {
    ok: /\{adapterError && \([\s\S]*?role="alert" className="mt-2 flex flex-wrap items-start justify-between gap-3[\s\S]*?网卡读取失败；当前列表仅供查看，DNS 服务器选择与设置已暂停：\{adapterError\}[\s\S]*?disabled=\{loading\} onClick=\{\(\) => void load\(\)\}[\s\S]*?loading \? "重试中…" : "重试"/.test(networkUi),
    description: "adapter read errors offer an inline, loading-aware retry while labeling stale adapter state read-only",
  },
  {
    ok: /if \(adapterResult\.status === "fulfilled"\)[\s\S]*setSel\(\(current\)[\s\S]*else \{[\s\S]*setAdapterError\(message\);[\s\S]*setSel\(null\);/.test(networkUi)
      && /disabled=\{loading \|\| adapterError !== null \|\| dnsAction !== null \|\| tcpBusy\}/.test(networkUi)
      && /disabled=\{!sel \|\| loading \|\| dnsAction !== null \|\| tcpBusy\}/.test(networkUi),
    description: "adapter read failures clear stale DNS selection and lock stale adapter rows until refresh succeeds",
  },
  {
    ok: /if \(busyRef\.current\.size > 0\) \{[\s\S]*?网卡设置正在应用，完成后自动刷新[\s\S]*?return;[\s\S]*?loadPending\.current = true;/.test(nicLatencyUi)
      && /if \(alive\.current\) setLoading\(true\);[\s\S]*Promise\.allSettled\(\[api\.listNicPower\(\), api\.listTcpLatency\(\)\]\)[\s\S]*setLoadError\(failures\.length \? "部分网卡设置读取失败；当前列表可能是上次成功数据，切换操作已暂停，请刷新后重试。" : null\)/.test(nicLatencyUi)
      && /const updateNic = async[\s\S]*?if \(loadPending\.current \|\| loadError \|\| busyRef\.current\.size > 0\) return;[\s\S]*?finally \{[\s\S]*?busyRef\.current\.delete\(key\);[\s\S]*?await load\(\);/.test(nicLatencyUi)
      && /const updateTcp = async[\s\S]*?if \(loadPending\.current \|\| loadError \|\| busyRef\.current\.size > 0\) return;[\s\S]*?finally \{[\s\S]*?busyRef\.current\.delete\(key\);[\s\S]*?await load\(\);/.test(nicLatencyUi)
      && /disabled=\{loading \|\| loadError !== null \|\| busy\.size > 0\}/.test(nicLatencyUi)
      && /disabled=\{loading \|\| busy\.size > 0\}/.test(nicLatencyUi)
      && /busy\.size > 0 \? "应用中…" : "刷新"/.test(nicLatencyUi)
      && /role="status" aria-live="polite" className="xl:col-span-2[\s\S]*网卡\/TCP 设置正在应用；完成后会自动重新读取状态。/.test(nicLatencyUi)
      && /\{loadError && \([\s\S]*?role="alert" className="mb-2 flex flex-wrap items-start justify-between gap-2[\s\S]*?disabled=\{loading\} onClick=\{\(\) => void load\(\)\}[\s\S]*?重试中…/.test(nicLatencyUi),
    description: "partial NIC/TCP reads fail closed, serialize writes, and refresh post-operation state with feedback",
  },
];

let failed = 0;
for (const check of checks) {
  console.log(`${check.ok ? "PASS" : "FAIL"} ${check.description}`);
  if (!check.ok) failed++;
}
if (failed) {
  console.error(`Network DNS contract failed: ${failed}/${checks.length}`);
  process.exitCode = 1;
} else {
  console.log(`Network DNS contract passed: ${checks.length} checks`);
}
