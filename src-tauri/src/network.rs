use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub desc: String,
    pub status: String,
    pub speed: String,
}

#[derive(Serialize, Clone)]
pub struct DnsEntry {
    pub iface: String,
    pub servers: Vec<String>,
}

fn json_list(raw: Option<String>) -> Vec<serde_json::Value> {
    let Some(raw) = raw else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return vec![];
    };
    match v {
        serde_json::Value::Array(a) => a,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => vec![],
    }
}

fn parse_adapters(raw: Option<String>) -> Vec<Adapter> {
    let mut adapters: Vec<Adapter> = json_list(raw)
        .into_iter()
        .map(|v| Adapter {
            name: v["Name"].as_str().unwrap_or("").into(),
            desc: v["InterfaceDescription"].as_str().unwrap_or("").into(),
            status: v["Status"].as_str().unwrap_or("").into(),
            speed: v["LinkSpeed"].as_str().unwrap_or("").into(),
        })
        .collect();
    adapters.sort_by_key(|adapter| (adapter.status != "Up", adapter.name.to_lowercase()));
    adapters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_parser_accepts_single_object_and_sorts_connected_first() {
        let one = parse_adapters(Some(
            r#"{"Name":"WLAN","Status":"Up","LinkSpeed":"1 Gbps"}"#.into(),
        ));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "WLAN");
        assert_eq!(one[0].desc, "");

        let many = parse_adapters(Some(r#"[{"Name":"Ethernet","Status":"Disconnected"},{"Name":"WLAN","Status":"Up"},{"Name":"Mihomo","Status":"Up"}]"#.into()));
        assert_eq!(
            many.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Mihomo", "WLAN", "Ethernet"]
        );
    }

    #[test]
    fn adapter_parser_degrades_malformed_or_null_payloads_to_empty() {
        assert!(parse_adapters(None).is_empty());
        assert!(parse_adapters(Some("null".into())).is_empty());
        assert!(parse_adapters(Some("not-json".into())).is_empty());
    }

    #[test]
    fn adapter_command_contract_keeps_command_failures_distinct_from_empty_result() {
        let result = Err::<Vec<Adapter>, String>("读取网卡失败".into());
        assert!(result.is_err());
        assert!(parse_adapters(Some("[]".into())).is_empty());
    }

    #[test]
    fn dns_input_accepts_ipv4_and_rejects_shell_syntax() {
        assert_eq!(
            parse_dns_servers(" 8.8.8.8 ", "1.1.1.1").unwrap(),
            vec!["8.8.8.8", "1.1.1.1"]
        );
        assert_eq!(
            parse_dns_servers("1.1.1.1", "1.1.1.1").unwrap(),
            vec!["1.1.1.1"]
        );
        for bad in [
            "",
            "999.1.1.1",
            "8.8.8.8'; Write-Output x",
            "https://example.test",
        ] {
            assert!(parse_dns_servers(bad, "").is_err(), "accepted {bad}");
        }
        assert!(parse_dns_servers("8.8.8.8", "1.1.1.1; Write-Output x").is_err());
    }

    #[test]
    fn dns_commands_quote_adapter_names_and_validate_addresses() {
        let alias = "Ethernet'; Write-Output 'injected";
        let script = build_dns_family_script(alias, &["8.8.8.8".into()], false).unwrap();
        assert!(script.contains("-InterfaceAlias 'Ethernet''; Write-Output ''injected'"));
        assert!(script.contains("-ServerAddresses @('8.8.8.8')"));
        assert!(!script.contains("-ServerAddresses @('8.8.8.8';"));
        assert!(build_dns_reset_script("Ethernet\nWrite-Output bad").is_err());
        assert!(
            build_dns_family_script("Ethernet", &["1.1.1.1; Write-Output x".into()], false)
                .is_err()
        );
    }

    #[test]
    fn dns_snapshot_parser_preserves_automatic_and_static_family_modes() {
        let settings =
            parse_dns_settings(r#"{"ipv4":["192.0.2.1","192.0.2.2"],"ipv6":null}"#).unwrap();
        assert_eq!(
            settings.ipv4,
            Some(vec!["192.0.2.1".into(), "192.0.2.2".into()])
        );
        assert_eq!(settings.ipv6, None);
        let ipv6 = parse_dns_settings(r#"{"ipv4":null,"ipv6":["2606:4700:4700::1111"]}"#).unwrap();
        assert_eq!(ipv6.ipv6, Some(vec!["2606:4700:4700::1111".into()]));
        assert!(parse_dns_settings(r#"{"ipv4":["not-an-ip"],"ipv6":null}"#).is_err());
    }

    #[test]
    fn dns_listing_parser_distinguishes_empty_from_malformed_results() {
        assert!(parse_dns_entries("[]").unwrap().is_empty());
        let one =
            parse_dns_entries(r#"{"InterfaceAlias":"Ethernet","ServerAddresses":["8.8.8.8"]}"#)
                .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].servers, vec!["8.8.8.8"]);
        assert!(parse_dns_entries("not-json").is_err());
        assert!(parse_dns_entries(
            r#"[{"InterfaceAlias":"Ethernet","ServerAddresses":["999.0.0.1"]}]"#
        )
        .is_err());
    }

    #[test]
    fn dns_restore_commands_reset_then_reapply_each_static_family() {
        let settings = crate::journal::DnsSettings {
            ipv4: None,
            ipv6: Some(vec!["2606:4700:4700::1111".into()]),
        };
        let scripts = build_dns_restore_scripts("Ethernet", &settings).unwrap();
        assert_eq!(scripts.len(), 2);
        assert!(scripts[0].contains("-ResetServerAddresses"));
        assert!(scripts[1].contains("-ServerAddresses @('2606:4700:4700::1111')"));
        assert!(build_dns_restore_scripts(
            "Ethernet",
            &crate::journal::DnsSettings {
                ipv4: Some(vec!["invalid".into()]),
                ipv6: None,
            }
        )
        .is_err());
    }

    #[test]
    #[ignore = "live read-only DNS configuration query"]
    fn live_dns_queries_return_typed_ipv4_and_rollback_snapshots() {
        let entries = query_dns_entries().expect("live DNS server query should return valid JSON");
        assert!(entries.iter().all(|entry| !entry.iface.is_empty()));
        let settings = read_dns_settings("WLAN").expect("live adapter DNS snapshot should parse");
        assert!(settings
            .ipv4
            .as_ref()
            .is_none_or(|servers| !servers.is_empty()));
    }

    #[test]
    fn live_windows_adapter_command_returns_parsed_adapters() {
        let adapters = list_adapters().expect("live Get-NetAdapter query should succeed");
        assert!(
            !adapters.is_empty(),
            "live Get-NetAdapter pipeline should return at least one adapter"
        );
        assert!(adapters
            .iter()
            .all(|a| !a.name.is_empty() && !a.status.is_empty()));
        assert!(
            adapters.iter().any(|a| a.name == "WLAN"),
            "expected the host WLAN adapter in the live result"
        );
    }
}

#[tauri::command]
pub fn list_adapters() -> Result<Vec<Adapter>, String> {
    let raw = crate::util::run_ps(
        "Get-NetAdapter | Sort-Object Status | Select-Object Name,InterfaceDescription,Status,LinkSpeed | ConvertTo-Json -Compress",
    ).ok_or_else(|| "读取网卡失败：PowerShell 未返回有效数据".to_string())?;
    let raw_len = raw.len();
    let adapters = parse_adapters(Some(raw));
    tracing::info!(raw_len, adapters = adapters.len(), "网卡查询完成");
    Ok(adapters)
}

fn normalize_ipv4_server(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 15 {
        return Err("DNS 服务器地址必须是有效 IPv4 地址".into());
    }
    value
        .parse::<std::net::Ipv4Addr>()
        .map(|address| address.to_string())
        .map_err(|_| "DNS 服务器地址必须是有效 IPv4 地址".to_string())
}

fn normalize_ipv6_server(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err("旧 IPv6 DNS 配置长度异常，已保留当前状态".into());
    }
    let (address, scope) = value.split_once('%').unwrap_or((value, ""));
    let normalized = address
        .parse::<std::net::Ipv6Addr>()
        .map_err(|_| "旧 IPv6 DNS 配置格式异常，已保留当前状态".to_string())?
        .to_string();
    if scope.is_empty() && value.contains('%') {
        return Err("旧 IPv6 DNS 配置的作用域格式异常，已保留当前状态".into());
    }
    if !scope.is_empty()
        && (!scope
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            || scope.len() > 32)
    {
        return Err("旧 IPv6 DNS 配置的作用域格式异常，已保留当前状态".into());
    }
    Ok(if scope.is_empty() {
        normalized
    } else {
        format!("{normalized}%{scope}")
    })
}

fn parse_address_values(value: &serde_json::Value) -> Result<Vec<String>, String> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::String(text) => Ok(text
            .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(String::from)
            .collect()),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|item| {
                item.as_str()
                    .map(String::from)
                    .ok_or_else(|| "DNS 服务器列表包含非文本项".to_string())
            })
            .collect(),
        _ => Err("DNS 服务器列表格式异常".into()),
    }
}

fn parse_dns_entries(raw: &str) -> Result<Vec<DnsEntry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("DNS 查询结果解析失败: {error}"))?;
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(row) => vec![serde_json::Value::Object(row)],
        _ => return Err("DNS 查询结果格式异常".into()),
    };
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let iface = row["InterfaceAlias"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if iface.is_empty() {
            return Err("DNS 查询结果缺少网卡名称".into());
        }
        let servers = parse_address_values(&row["ServerAddresses"])?
            .iter()
            .map(|address| normalize_ipv4_server(address))
            .collect::<Result<Vec<_>, _>>()?;
        entries.push(DnsEntry { iface, servers });
    }
    entries.sort_by_key(|entry| entry.iface.to_lowercase());
    Ok(entries)
}

fn query_dns_entries() -> Result<Vec<DnsEntry>, String> {
    let script = "$ErrorActionPreference='Stop'; $items=@(Get-DnsClientServerAddress -AddressFamily IPv4 -ErrorAction Stop | Where-Object { $_.ServerAddresses.Count -gt 0 } | Select-Object InterfaceAlias,ServerAddresses); ConvertTo-Json -InputObject $items -Compress -Depth 4";
    let raw = crate::util::run_ps_with_timeout_result(script, std::time::Duration::from_secs(10))?;
    parse_dns_entries(&raw)
}

#[tauri::command]
pub fn get_dns() -> Result<Vec<DnsEntry>, String> {
    query_dns_entries()
}

fn validate_interface_alias(iface: &str) -> Result<String, String> {
    let iface = iface.trim();
    if iface.is_empty() || iface.chars().count() > 256 || iface.chars().any(char::is_control) {
        return Err("网卡名称为空或包含不支持的字符".into());
    }
    Ok(iface.to_string())
}

fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn parse_dns_servers(primary: &str, secondary: &str) -> Result<Vec<String>, String> {
    let primary = normalize_ipv4_server(primary)?;
    let secondary = secondary.trim();
    let mut servers = vec![primary.clone()];
    if !secondary.is_empty() {
        let secondary = normalize_ipv4_server(secondary)?;
        if secondary != primary {
            servers.push(secondary);
        }
    }
    Ok(servers)
}

fn parse_dns_settings(raw: &str) -> Result<crate::journal::DnsSettings, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("DNS 恢复快照解析失败: {error}"))?;
    if !value.is_object() {
        return Err("DNS 恢复快照格式异常".into());
    }
    let parse_family = |key: &str, ipv6: bool| -> Result<Option<Vec<String>>, String> {
        let addresses = parse_address_values(&value[key])?;
        if addresses.is_empty() {
            return Ok(None);
        }
        if addresses.len() > 16 {
            return Err("DNS 恢复快照服务器数量异常，已保留当前状态".into());
        }
        let normalized = addresses
            .iter()
            .map(|address| {
                if ipv6 {
                    normalize_ipv6_server(address)
                } else {
                    normalize_ipv4_server(address)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(normalized))
    };
    Ok(crate::journal::DnsSettings {
        ipv4: parse_family("ipv4", false)?,
        ipv6: parse_family("ipv6", true)?,
    })
}

pub(crate) fn read_dns_settings(iface: &str) -> Result<crate::journal::DnsSettings, String> {
    let iface = validate_interface_alias(iface)?;
    let script = r#"
$ErrorActionPreference = 'Stop'
$target = __IFACE__
$adapter = @(Get-NetAdapter -ErrorAction Stop | Where-Object { $_.Name -eq $target } | Select-Object -First 1)
if ($adapter.Count -ne 1) { throw '网卡未找到或不唯一' }
$guid = '{' + ([string]$adapter[0].InterfaceGuid).Trim('{}') + '}'
$path4 = "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\$guid"
$path6 = "HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters\Interfaces\$guid"
$props4 = if (Test-Path -LiteralPath $path4) { Get-ItemProperty -LiteralPath $path4 -ErrorAction Stop } else { $null }
$props6 = if (Test-Path -LiteralPath $path6) { Get-ItemProperty -LiteralPath $path6 -ErrorAction Stop } else { $null }
function Split-StaticServers([string]$value) {
  if ([string]::IsNullOrWhiteSpace($value)) { return $null }
  $items = @($value -split '[,;\s]+' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if ($items.Count -eq 0) { return $null }
  return ,$items
}
$payload = [pscustomobject]@{
  ipv4 = (Split-StaticServers ([string]$props4.NameServer))
  ipv6 = (Split-StaticServers ([string]$props6.NameServer))
}
ConvertTo-Json -InputObject $payload -Compress -Depth 3
"#
    .replace("__IFACE__", &powershell_single_quote(&iface));
    let raw = crate::util::run_ps_with_timeout_result(&script, std::time::Duration::from_secs(10))?;
    parse_dns_settings(&raw)
}

fn build_dns_reset_script(iface: &str) -> Result<String, String> {
    let iface = powershell_single_quote(&validate_interface_alias(iface)?);
    Ok(format!("$ErrorActionPreference='Stop'; Set-DnsClientServerAddress -InterfaceAlias {iface} -ResetServerAddresses -ErrorAction Stop"))
}

fn build_dns_family_script(iface: &str, servers: &[String], ipv6: bool) -> Result<String, String> {
    let iface = powershell_single_quote(&validate_interface_alias(iface)?);
    if servers.is_empty() || servers.len() > 16 {
        return Err("DNS 服务器列表数量异常".into());
    }
    let servers = servers
        .iter()
        .map(|server| {
            let normalized = if ipv6 {
                normalize_ipv6_server(server)?
            } else {
                normalize_ipv4_server(server)?
            };
            Ok(powershell_single_quote(&normalized))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join(",");
    Ok(format!("$ErrorActionPreference='Stop'; Set-DnsClientServerAddress -InterfaceAlias {iface} -ServerAddresses @({servers}) -ErrorAction Stop"))
}

fn build_dns_restore_scripts(
    iface: &str,
    settings: &crate::journal::DnsSettings,
) -> Result<Vec<String>, String> {
    let mut scripts = vec![build_dns_reset_script(iface)?];
    if let Some(servers) = &settings.ipv4 {
        scripts.push(build_dns_family_script(iface, servers, false)?);
    }
    if let Some(servers) = &settings.ipv6 {
        scripts.push(build_dns_family_script(iface, servers, true)?);
    }
    Ok(scripts)
}

fn run_dns_script(script: &str) -> Result<(), String> {
    crate::util::run_checked(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}

fn set_dns_v4_raw(iface: &str, servers: &[String]) -> Result<(), String> {
    run_dns_script(&build_dns_family_script(iface, servers, false)?)
}

pub(crate) fn apply_dns_settings(
    iface: &str,
    settings: &crate::journal::DnsSettings,
) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("还原 DNS 需要管理员权限".into());
    }
    for script in build_dns_restore_scripts(iface, settings)? {
        run_dns_script(&script).map_err(|error| format!("DNS 配置还原失败: {error}"))?;
    }
    Ok(())
}

/// Set the IPv4 DNS servers after validating both addresses and snapshotting the original v4/v6 modes.
#[tauri::command]
pub fn set_dns(iface: String, primary: String, secondary: String) -> Result<(), String> {
    let iface = validate_interface_alias(&iface)?;
    let servers = parse_dns_servers(&primary, &secondary)?;
    if !crate::util::is_admin() {
        return Err("设置 DNS 需要管理员权限，请在「设置」页提权后重试".into());
    }
    let old = read_dns_settings(&iface)?;
    let mut applied = old.clone();
    applied.ipv4 = Some(servers.clone());
    if old == applied {
        return Ok(());
    }
    set_dns_v4_raw(&iface, &servers)?;
    crate::journal::record(
        &format!("DNS 设置 → {iface}"),
        crate::journal::Revert::Dns {
            iface,
            old,
            applied,
        },
    )?;
    Ok(())
}

#[tauri::command]
pub fn reset_dns(iface: String) -> Result<(), String> {
    let iface = validate_interface_alias(&iface)?;
    if !crate::util::is_admin() {
        return Err("重置 DNS 需要管理员权限".into());
    }
    let old = read_dns_settings(&iface)?;
    let applied = crate::journal::DnsSettings::default();
    if old == applied {
        return Ok(());
    }
    run_dns_script(&build_dns_reset_script(&iface)?)?;
    crate::journal::record(
        &format!("恢复 DNS 自动获取 → {iface}"),
        crate::journal::Revert::Dns {
            iface,
            old,
            applied,
        },
    )?;
    Ok(())
}

#[tauri::command]
pub fn flush_dns() -> Result<(), String> {
    crate::util::run_checked("ipconfig", &["/flushdns"])
}

const TCP_AUTOTUNING_LEVELS: [&str; 5] = [
    "disabled",
    "highlyrestricted",
    "restricted",
    "normal",
    "experimental",
];

/// 仅接受变更日志恢复器可安全执行的 TCP 自动调节级别。
pub(crate) fn normalize_tcp_autotuning_level(level: &str) -> Option<String> {
    let normalized = level.trim().to_ascii_lowercase();
    TCP_AUTOTUNING_LEVELS
        .contains(&normalized.as_str())
        .then_some(normalized)
}

fn set_tcp_autotuning_with(
    requested: &str,
    read_current: impl FnOnce() -> String,
    apply: impl FnOnce(&str) -> Result<(), String>,
    record: impl FnOnce(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    let requested = normalize_tcp_autotuning_level(requested)
        .ok_or_else(|| format!("不支持的级别: {requested}"))?;
    let current = read_current();
    let old = normalize_tcp_autotuning_level(&current).ok_or_else(|| {
        format!("当前 TCP 自动调节级别不可读取或无法安全还原，未应用更改: {current}")
    })?;
    apply(&requested)?;
    record(&old, &requested)
}

#[tauri::command]
pub fn get_tcp_autotuning() -> String {
    crate::util::run_ps("(Get-NetTCPSetting -SettingName Internet).AutoTuningLevelLocal")
        .map(|level| level.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".into())
}

/// 设置 TCP 接收窗口自动调节级别（normal 是推荐值）。记录可还原变更。
#[tauri::command]
pub fn set_tcp_autotuning(level: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("TCP 调优需要管理员权限，请在「设置」页提权后重试".into());
    }
    set_tcp_autotuning_with(
        &level,
        get_tcp_autotuning,
        |requested| {
            let command = format!(
                "Set-NetTCPSetting -SettingName Internet -AutoTuningLevelLocal {requested}"
            );
            crate::util::run_checked(
                "powershell",
                &["-NoProfile", "-NonInteractive", "-Command", &command],
            )
        },
        |old, applied| {
            crate::journal::record(
                &format!("TCP 接收窗口自动调节 → {applied}"),
                crate::journal::Revert::Tcp {
                    level: old.to_owned(),
                    applied_level: Some(applied.to_owned()),
                },
            )
        },
    )
}

#[derive(Serialize, Clone)]
pub struct WifiProfile {
    #[serde(rename = "ssid")]
    pub name: String,
    pub auth: String,
    pub cipher: String,
}

fn parse_wifi_profiles(raw: Option<String>) -> Vec<WifiProfile> {
    json_list(raw)
        .into_iter()
        .map(|v| WifiProfile {
            name: v["ssid"]
                .as_str()
                .or_else(|| v["name"].as_str())
                .unwrap_or("")
                .to_string(),
            auth: v["auth"].as_str().unwrap_or("").to_string(),
            cipher: v["cipher"].as_str().unwrap_or("").to_string(),
        })
        .filter(|p| !p.name.is_empty())
        .collect()
}

#[tauri::command]
pub fn list_wifi_profiles() -> Vec<WifiProfile> {
    // WLAN XML is language-independent. Never request key=clear: this feature only
    // displays profile metadata, not saved credentials. Remove temporary exports in finally.
    let script = r#"
$ErrorActionPreference='Stop'
$dir=Join-Path $env:TEMP ('aerotune-wlan-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $dir | Out-Null
try {
  netsh wlan export profile folder="$dir" | Out-Null
  Get-ChildItem -LiteralPath $dir -Filter '*.xml' | ForEach-Object {
    [xml]$x=Get-Content -LiteralPath $_.FullName -Raw
    $ns=New-Object System.Xml.XmlNamespaceManager($x.NameTable); $ns.AddNamespace('w','http://www.microsoft.com/networking/WLAN/profile/v1')
    $name=$x.SelectSingleNode('/w:WLANProfile/w:name',$ns).InnerText
    $auth=$x.SelectSingleNode('//w:authentication',$ns).InnerText
    $cipher=$x.SelectSingleNode('//w:encryption',$ns).InnerText
    [pscustomobject]@{ssid=$name;auth=$auth;cipher=$cipher}
  } | ConvertTo-Json -Compress
} finally { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
"#;
    parse_wifi_profiles(crate::util::run_ps(script))
}

#[derive(Serialize, Clone)]
pub struct NicPower {
    pub name: String,
    pub guid: String,
    pub pm_disabled: bool,
}

#[derive(Serialize, Clone)]
pub struct TcpLatency {
    pub name: String,
    pub guid: String,
    pub ack_freq: Option<u32>,
    pub nodelay: Option<u32>,
}

fn parse_nic_power(raw: Option<String>) -> Vec<NicPower> {
    json_list(raw)
        .into_iter()
        .filter_map(|v| {
            let guid = normalize_adapter_guid(v["guid"].as_str()?)?;
            Some(NicPower {
                name: v["name"].as_str().unwrap_or("").to_string(),
                guid,
                pm_disabled: v["PnPCapabilities"].as_u64().unwrap_or(0) == 24,
            })
        })
        .collect()
}

fn parse_tcp_latency(raw: Option<String>) -> Vec<TcpLatency> {
    json_list(raw)
        .into_iter()
        .filter_map(|v| {
            let guid = normalize_adapter_guid(v["guid"].as_str()?)?;
            Some(TcpLatency {
                name: v["name"].as_str().unwrap_or("").to_string(),
                guid,
                ack_freq: v["TcpAckFrequency"].as_u64().map(|n| n as u32),
                nodelay: v["TCPNoDelay"].as_u64().map(|n| n as u32),
            })
        })
        .collect()
}

pub(crate) fn normalize_adapter_guid(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_start_matches('{').trim_end_matches('}');
    let compact: String = trimmed.chars().filter(|c| *c != '-').collect();
    if compact.len() != 32 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{{{}}}", trimmed.to_ascii_uppercase()))
}

fn nic_power_path(guid: &str) -> Result<String, String> {
    let guid = normalize_adapter_guid(guid).ok_or_else(|| "网卡 GUID 不合法".to_string())?;
    let raw = crate::util::run_ps(&format!(
        "$g='{}'; $k=Get-ChildItem 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Class\\{{4d36e972-e325-11ce-bfc1-08002be10318}}' | Where-Object {{ (Get-ItemProperty $_.PSPath -Name NetCfgInstanceId -ErrorAction SilentlyContinue).NetCfgInstanceId -eq $g }} | Select-Object -First 1 -ExpandProperty PSPath; if($k){{$k}}",
        guid
    )).ok_or_else(|| "找不到网卡注册表配置".to_string())?;
    if !raw.starts_with("Microsoft.PowerShell.Core\\Registry::HKEY_LOCAL_MACHINE\\")
        && !raw.starts_with("HKLM:\\")
    {
        return Err("找不到网卡注册表配置".into());
    }
    let suffix = raw
        .strip_prefix("Microsoft.PowerShell.Core\\Registry::HKEY_LOCAL_MACHINE\\")
        .or_else(|| raw.strip_prefix("HKLM:\\"))
        .ok_or_else(|| "网卡注册表路径无效".to_string())?;
    Ok(format!("HKLM\\{suffix}"))
}

fn build_nic_guid_validation_script(expected_guid: &str) -> Option<String> {
    let expected_guid = normalize_adapter_guid(expected_guid)?;
    Some(format!(
        r#"$expected='{expected_guid}';try{{$a=@(Get-NetAdapter -ErrorAction Stop | Where-Object{{('{{'+$_.InterfaceGuid.ToString().ToUpper()+'}}') -eq $expected}});if($a.Count -eq 1){{'true'}}else{{'false'}}}}catch{{'false'}}"#
    ))
}

/// Confirm the captured adapter GUID is still represented by exactly one enumerated NIC.
pub(crate) fn nic_guid_is_uniquely_enumerated(expected_guid: &str) -> bool {
    let Some(script) = build_nic_guid_validation_script(expected_guid) else {
        return false;
    };
    crate::util::run_ps(&script).is_some_and(|result| result.trim().eq_ignore_ascii_case("true"))
}

fn tcp_path(guid: &str) -> Result<String, String> {
    let guid = normalize_adapter_guid(guid).ok_or_else(|| "网卡 GUID 不合法".to_string())?;
    Ok(format!(
        "HKLM\\SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters\\Interfaces\\{guid}"
    ))
}

#[tauri::command]
pub fn list_nic_power() -> Vec<NicPower> {
    let script = r#"Get-NetAdapter | ForEach-Object { $g='{'+$_.InterfaceGuid.ToString().ToUpper()+'}'; $p=Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}' | Where-Object { (Get-ItemProperty $_.PSPath -Name NetCfgInstanceId -ErrorAction SilentlyContinue).NetCfgInstanceId -eq $g } | Select-Object -First 1; $v=if($p){(Get-ItemProperty $p.PSPath -Name PnPCapabilities -ErrorAction SilentlyContinue).PnPCapabilities}else{$null}; [pscustomobject]@{name=$_.Name;guid=$g;PnPCapabilities=$v} } | ConvertTo-Json -Compress"#;
    parse_nic_power(crate::util::run_ps(script))
}

#[tauri::command]
pub fn set_nic_power(guid: String, disable: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("修改网卡电源管理需要管理员权限".into());
    }
    let expected_guid =
        normalize_adapter_guid(&guid).ok_or_else(|| "网卡 GUID 不合法".to_string())?;
    let path = nic_power_path(&expected_guid)?;
    if disable {
        crate::journal::set_nic_reg_dword(
            &path,
            "PnPCapabilities",
            24,
            "禁用网卡节能",
            &expected_guid,
        )
    } else {
        crate::journal::del_nic_reg_value(
            &path,
            "PnPCapabilities",
            "恢复网卡默认电源管理",
            &expected_guid,
        )
    }
}

#[tauri::command]
pub fn list_tcp_latency() -> Vec<TcpLatency> {
    let script = r#"Get-NetAdapter | ForEach-Object { $g='{'+$_.InterfaceGuid.ToString().ToUpper()+'}'; $p='HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\'+$g; $v=Get-ItemProperty $p -ErrorAction SilentlyContinue; [pscustomobject]@{name=$_.Name;guid=$g;TcpAckFrequency=$v.TcpAckFrequency;TCPNoDelay=$v.TCPNoDelay} } | ConvertTo-Json -Compress"#;
    parse_tcp_latency(crate::util::run_ps(script))
}

#[tauri::command]
pub fn set_tcp_latency(guid: String, on: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("TCP 延迟优化需要管理员权限".into());
    }
    let path = tcp_path(&guid)?;
    if on {
        crate::journal::set_reg_dword(
            &path,
            "TcpAckFrequency",
            1,
            "启用 TCP 延迟优化（ACK Frequency）",
        )?;
        crate::journal::set_reg_dword(&path, "TCPNoDelay", 1, "启用 TCP 延迟优化（NoDelay）")
    } else {
        crate::journal::del_reg_value(&path, "TCPNoDelay", "还原 TCP 延迟优化（NoDelay）")?;
        crate::journal::del_reg_value(
            &path,
            "TcpAckFrequency",
            "还原 TCP 延迟优化（ACK Frequency）",
        )
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct PingStat {
    ok: u32,
    avg: Option<f64>,
    sd: Option<f64>,
    loss_pct: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct NetQualitySummary {
    pub label: String,
    pub detail: String,
    pub tone: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct NetQuality {
    gateway: Option<PingStat>,
    dns: PingStat,
    second_dns: Option<PingStat>,
    summary: NetQualitySummary,
}

fn classify_net_quality(
    gateway: Option<&PingStat>,
    dns: &PingStat,
    second_dns: Option<&PingStat>,
) -> NetQualitySummary {
    let unreachable = [gateway, Some(dns), second_dns]
        .into_iter()
        .flatten()
        .any(|s| s.ok == 0);
    let gateway_bad = gateway.is_some_and(|s| {
        s.loss_pct >= 25.0 || s.avg.unwrap_or(0.0) >= 80.0 || s.sd.unwrap_or(0.0) >= 30.0
    });
    let dns_bad = [Some(dns), second_dns].into_iter().flatten().any(|s| {
        s.loss_pct >= 25.0 || s.avg.unwrap_or(0.0) >= 120.0 || s.sd.unwrap_or(0.0) >= 50.0
    });
    let (label, detail, tone) = if unreachable {
        (
            "存在不可达目标",
            "可能是 ICMP 被过滤；结合网关与 DNS 各项结果判断，不代表网页服务一定不可用。",
            "rose",
        )
    } else if gateway_bad {
        (
            "本地链路需关注",
            "网关延迟、抖动或丢包偏高；优先检查 Wi-Fi 信号、网线与本地路由设备。",
            "amber",
        )
    } else if dns_bad {
        (
            "外部链路或 DNS 需关注",
            "网关相对正常但公共 DNS 探测较慢；可对比不同 DNS，并结合实际网页解析表现。",
            "amber",
        )
    } else if gateway.is_none() {
        (
            "网关探测不可用",
            "未发现默认网关或路由信息；仅显示公共 DNS 探测结果。",
            "amber",
        )
    } else {
        (
            "探测结果良好",
            "本次 ICMP 样本未见明显延迟或丢包；小样本用于快速定位，建议与实际应用体验结合。",
            "green",
        )
    };
    NetQualitySummary {
        label: label.into(),
        detail: detail.into(),
        tone: tone.into(),
    }
}

fn parse_ping_stat(value: &serde_json::Value) -> Option<PingStat> {
    let ok = u32::try_from(value["ok"].as_u64()?).ok()?;
    let avg = value["avg"].as_f64();
    let sd = value["sd"].as_f64();
    let loss_pct = value["loss_pct"].as_f64()?;
    if ok > 4
        || !(0.0..=100.0).contains(&loss_pct)
        || avg.is_some_and(|v| !v.is_finite() || v < 0.0)
        || sd.is_some_and(|v| !v.is_finite() || v < 0.0)
        || (ok == 0 && (avg.is_some() || sd.is_some()))
        || (ok > 0 && (avg.is_none() || sd.is_none()))
    {
        return None;
    }
    Some(PingStat {
        ok,
        avg,
        sd,
        loss_pct,
    })
}

fn parse_net_quality(raw: Result<String, String>) -> Result<NetQuality, String> {
    let raw = raw?;
    let value: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|_| "网络探测输出不是有效 JSON".to_string())?;
    let second_dns = if value["second_dns"].is_null() {
        None
    } else {
        Some(
            parse_ping_stat(&value["second_dns"])
                .ok_or_else(|| "网络探测输出中的第二个 DNS 统计无效".to_string())?,
        )
    };
    let gateway = if value["gateway"].is_null() {
        None
    } else {
        Some(
            parse_ping_stat(&value["gateway"])
                .ok_or_else(|| "网络探测输出中的网关统计无效".to_string())?,
        )
    };
    let dns = parse_ping_stat(&value["dns"])
        .ok_or_else(|| "网络探测输出中的 DNS 统计无效".to_string())?;
    let summary = classify_net_quality(gateway.as_ref(), &dns, second_dns.as_ref());
    Ok(NetQuality {
        gateway,
        dns,
        second_dns,
        summary,
    })
}

#[tauri::command]
pub fn ping_quality() -> Result<NetQuality, String> {
    let script = r#"
$ErrorActionPreference='SilentlyContinue'
function Probe([string]$Target) {
  if(-not $Target){ return $null }
  $r=@(Test-Connection -ComputerName $Target -Count 4 -ErrorAction SilentlyContinue | ForEach-Object { if($_.Latency -ne $null){[double]$_.Latency}else{[double]$_.ResponseTime} })
  $ok=$r.Count; $avg=$null; $sd=$null
  if($ok -gt 0){$avg=($r|Measure-Object -Average).Average; $variance=0.0; foreach($n in $r){$variance += [math]::Pow(($n-$avg),2)}; $sd=[math]::Sqrt($variance/$ok)}
  [pscustomobject]@{ok=$ok;avg=$avg;sd=$sd;loss_pct=[math]::Round((4-$ok)*25,1)}
}
$gw=(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric | Select-Object -First 1).NextHop
[pscustomobject]@{gateway=(Probe $gw);dns=(Probe '223.5.5.5');second_dns=(Probe '119.29.29.29')} | ConvertTo-Json -Compress -Depth 4
"#;
    parse_net_quality(crate::util::run_ps_with_timeout_result(
        script,
        std::time::Duration::from_secs(60),
    ))
}

#[cfg(test)]
mod restored_tests {
    use super::*;

    #[test]
    fn tcp_autotuning_levels_match_the_restore_allowlist() {
        for level in [
            "disabled",
            "highlyrestricted",
            "restricted",
            "normal",
            "experimental",
        ] {
            assert_eq!(
                normalize_tcp_autotuning_level(level).as_deref(),
                Some(level)
            );
        }
        assert_eq!(
            normalize_tcp_autotuning_level(" Normal\r\n").as_deref(),
            Some("normal")
        );
        for level in [
            "unknown",
            "futurelevel",
            "normal; Write-Output injected",
            "",
        ] {
            assert!(normalize_tcp_autotuning_level(level).is_none());
        }
    }

    #[test]
    fn tcp_autotuning_rejects_unrestorable_old_state_before_mutation() {
        let mut applied = false;
        let mut recorded = false;
        let result = set_tcp_autotuning_with(
            "normal",
            || "unknown".into(),
            |_| {
                applied = true;
                Ok(())
            },
            |_, _| {
                recorded = true;
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("未应用更改"));
        assert!(!applied);
        assert!(!recorded);
    }

    #[test]
    fn tcp_autotuning_records_a_normalized_restorable_snapshot() {
        let mut applied = None;
        let mut recorded = None;
        set_tcp_autotuning_with(
            "EXPERIMENTAL",
            || "Normal".into(),
            |requested| {
                applied = Some(requested.to_string());
                Ok(())
            },
            |old, next| {
                recorded = Some((old.to_string(), next.to_string()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(applied.as_deref(), Some("experimental"));
        assert_eq!(recorded, Some(("normal".into(), "experimental".into())));
    }

    #[test]
    fn wifi_parser_maps_metadata_without_retaining_credential_fields() {
        let parsed = parse_wifi_profiles(Some(r#"[{"ssid":"home","auth":"WPA2PSK","cipher":"AES","password":"secret"},{"ssid":"guest","auth":"open"},{"auth":"WPA2"}]"#.into()));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "home");
        assert_eq!(parsed[0].auth, "WPA2PSK");
        assert_eq!(parsed[0].cipher, "AES");
        let json = serde_json::to_value(&parsed[1]).unwrap();
        assert_eq!(json["ssid"], "guest");
        assert!(json.get("password").is_none());
        assert!(!serde_json::to_string(&parsed).unwrap().contains("secret"));
    }

    #[test]
    fn guid_and_registry_path_validation_are_strict() {
        assert_eq!(
            normalize_adapter_guid("{01234567-89ab-cdef-0123-456789abcdef}"),
            Some("{01234567-89AB-CDEF-0123-456789ABCDEF}".into())
        );
        assert!(normalize_adapter_guid("not-a-guid").is_none());
        let expected_guid = "{01234567-89ab-cdef-0123-456789abcdef}";
        let script = build_nic_guid_validation_script(expected_guid).unwrap();
        assert!(script.contains("Get-NetAdapter -ErrorAction Stop"));
        assert!(script.contains("$expected='{01234567-89AB-CDEF-0123-456789ABCDEF}'"));
        assert!(script.contains("-eq $expected"));
        assert!(
            build_nic_guid_validation_script("{01234567-89ab-cdef-0123-456789abcdef}';exit")
                .is_none()
        );
        assert!(build_nic_guid_validation_script("not-a-guid").is_none());
        assert!(tcp_path("bad").is_err());
        assert!(tcp_path("01234567-89ab-cdef-0123-456789abcdef")
            .unwrap()
            .ends_with("{01234567-89AB-CDEF-0123-456789ABCDEF}"));
    }

    #[test]
    fn nic_and_latency_parsers_handle_optional_values() {
        let nic = parse_nic_power(Some(r#"[{"name":"Wi-Fi","guid":"{01234567-89ab-cdef-0123-456789abcdef}","PnPCapabilities":24}]"#.into()));
        assert_eq!(nic.len(), 1);
        assert!(nic[0].pm_disabled);
        let tcp = parse_tcp_latency(Some(
            r#"{"name":"Wi-Fi","guid":"01234567-89ab-cdef-0123-456789abcdef","TcpAckFrequency":1}"#
                .into(),
        ));
        assert_eq!(tcp.len(), 1);
        assert_eq!(tcp[0].ack_freq, Some(1));
        assert_eq!(tcp[0].nodelay, None);
    }

    #[test]
    fn ping_parser_preserves_nullable_gateway_optional_second_dns_and_empty_result() {
        let parsed = parse_net_quality(Ok(r#"{"gateway":null,"dns":{"ok":2,"avg":5.0,"sd":1.0,"loss_pct":50.0},"second_dns":{"ok":4,"avg":10.0,"sd":2.0,"loss_pct":0.0}}"#.into())).unwrap();
        assert_eq!(parsed.gateway, None);
        assert_eq!(parsed.dns.ok, 2);
        assert_eq!(parsed.second_dns.as_ref().unwrap().ok, 4);
        let old = parse_net_quality(Ok(
            r#"{"gateway":null,"dns":{"ok":0,"avg":null,"sd":null,"loss_pct":100.0}}"#.into(),
        ))
        .unwrap();
        assert_eq!(old.second_dns, None);
        assert_eq!(
            serde_json::to_value(&old).unwrap()["second_dns"],
            serde_json::Value::Null
        );
        assert!(parse_net_quality(Ok("bad json".into())).is_err());
    }

    #[test]
    fn ping_probe_errors_do_not_become_packet_loss_measurements() {
        let timeout = parse_net_quality(Err("PowerShell 查询超时".into()));
        assert_eq!(timeout.unwrap_err(), "PowerShell 查询超时");
        assert!(parse_net_quality(Ok("malformed JSON".into())).is_err());

        let measured_unreachable = parse_net_quality(Ok(
            r#"{"gateway":{"ok":0,"avg":null,"sd":null,"loss_pct":100.0},"dns":{"ok":0,"avg":null,"sd":null,"loss_pct":100.0},"second_dns":{"ok":0,"avg":null,"sd":null,"loss_pct":100.0}}"#.into(),
        ))
        .unwrap();
        assert_eq!(measured_unreachable.dns.ok, 0);
        assert_eq!(measured_unreachable.dns.loss_pct, 100.0);
        assert_eq!(measured_unreachable.summary.label, "存在不可达目标");
    }

    #[test]
    fn route_summary_localizes_gateway_dns_and_unreachable_cases() {
        let stat = |avg, sd, loss| PingStat {
            ok: 4,
            avg: Some(avg),
            sd: Some(sd),
            loss_pct: loss,
        };
        let dns = stat(15.0, 2.0, 0.0);
        assert_eq!(
            classify_net_quality(Some(&stat(100.0, 3.0, 0.0)), &dns, None).label,
            "本地链路需关注"
        );
        assert_eq!(
            classify_net_quality(Some(&stat(10.0, 2.0, 0.0)), &stat(180.0, 3.0, 0.0), None).label,
            "外部链路或 DNS 需关注"
        );
        assert_eq!(
            classify_net_quality(None, &dns, None).label,
            "网关探测不可用"
        );
        let failed = PingStat {
            ok: 0,
            avg: None,
            sd: None,
            loss_pct: 100.0,
        };
        assert_eq!(classify_net_quality(Some(&failed), &dns, None).tone, "rose");
    }

    #[test]
    fn second_dns_malformed_data_invalidates_instead_of_hiding_bad_measurement() {
        assert!(parse_net_quality(Ok(r#"{"dns":{"ok":4,"avg":10.0,"sd":1.0,"loss_pct":0.0},"second_dns":{"ok":9,"avg":1.0,"sd":0.0,"loss_pct":0.0}}"#.into())).is_err());
    }

    #[test]
    fn ping_parser_rejects_impossible_counts_and_metrics() {
        for json in [
            r#"{"dns":{"ok":5,"avg":2.0,"sd":0.0,"loss_pct":0.0}}"#,
            r#"{"dns":{"ok":1,"avg":-1.0,"sd":0.0,"loss_pct":0.0}}"#,
            r#"{"dns":{"ok":0,"avg":null,"sd":null,"loss_pct":101.0}}"#,
            r#"{"dns":{"ok":1,"avg":1.0,"sd":null,"loss_pct":75.0}}"#,
        ] {
            assert!(
                parse_net_quality(Ok(json.into())).is_err(),
                "accepted {json}"
            );
        }
    }
}
