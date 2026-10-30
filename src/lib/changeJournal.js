const CATEGORY_LABELS = {
  registry: "注册表",
  power_plan: "电源",
  tcp: "TCP",
  dns: "DNS",
};

const TCP_LEVELS = new Set(["disabled", "highlyrestricted", "restricted", "normal", "experimental"]);
const GUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_TARGET_PART_LENGTH = 512;
const MAX_CONFIRMATION_CHARS = 16_000;
const NIC_PNP_PREFIXES = [
  "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\",
  "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\",
];
const NIC_PNP_KEY_PATTERN = /^(?:HKLM|HKEY_LOCAL_MACHINE)\\SYSTEM\\CurrentControlSet\\Control\\Class\\\{4d36e972-e325-11ce-bfc1-08002be10318\}\\\d{4}$/i;
const NIC_GUID_PATTERN = /^\{[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\}$/i;

export function getChangeCategory(type) {
  return Object.hasOwn(CATEGORY_LABELS, type) ? CATEGORY_LABELS[type] : "其他";
}

function safeTargetPart(value) {
  const supplied = value !== null && value !== undefined && String(value).trim() !== "";
  const raw = supplied ? String(value) : "未知";
  const hasControls = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(raw);
  const clean = raw.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " ").replace(/\s+/g, " ").trim() || "未知";
  // Rust bounds user-visible adapter aliases in Unicode scalar values, not UTF-16 code units.
  const preview = Array.from(clean.slice(0, MAX_TARGET_PART_LENGTH * 2 + 2));
  const truncated = preview.length > MAX_TARGET_PART_LENGTH;
  return {
    text: `${preview.slice(0, MAX_TARGET_PART_LENGTH).join("")}${truncated ? "…" : ""}`,
    complete: supplied && !hasControls && !truncated,
  };
}

function describeRegistrySnapshot(snapshot) {
  if (!snapshot || typeof snapshot !== "object") return { text: "原值未知", complete: false };
  if (snapshot.k === "missing") return { text: "删除值（原本不存在）", complete: true };
  if (snapshot.k === "dword") {
    const value = Number.isInteger(snapshot.value) ? snapshot.value : snapshot["0"];
    return Number.isInteger(value) && value >= 0 && value <= 0xffffffff
      ? { text: `DWORD ${value} (0x${value.toString(16).padStart(8, "0")})`, complete: true }
      : { text: "DWORD 值未知", complete: false };
  }
  if (snapshot.k === "sz") {
    const value = typeof snapshot.value === "string" ? snapshot.value : snapshot["0"];
    return typeof value === "string"
      ? { text: `REG_SZ（${value.length} 字符，内容不显示）`, complete: true }
      : { text: "REG_SZ 值未知", complete: false };
  }
  if (snapshot.k === "raw" && Number.isInteger(snapshot.vtype) && Array.isArray(snapshot.bytes)) {
    const bytes = snapshot.bytes;
    if (bytes.length > 4096 || !bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255)) {
      return { text: "原值快照过大或格式无效", complete: false };
    }
    if (snapshot.vtype === 4) {
      if (bytes.length !== 4) return { text: "DWORD 快照格式无效", complete: false };
      const value = (bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24)) >>> 0;
      return { text: `DWORD ${value} (0x${value.toString(16).padStart(8, "0")})`, complete: true };
    }
    return { text: `注册表类型 ${snapshot.vtype}（${bytes.length} 字节，内容不显示）`, complete: true };
  }
  return { text: "原值格式未知", complete: false };
}

function withinUnicodeCharacterLimit(value, limit) {
  return typeof value === "string" && value.length <= limit * 2 && Array.from(value).length <= limit;
}

function describeDnsFamily(value, family) {
  if (value === null) return { text: `${family} 自动获取`, complete: true };
  if (!Array.isArray(value)) return { text: `${family} 状态未知`, complete: false };
  if (value.length > 16) return { text: `${family} 服务器超过 16 项（共 ${value.length} 项）`, complete: false };
  if (value.length === 0) return { text: `${family} 静态空列表`, complete: true };
  const maxAddressLength = family === "IPv4" ? 15 : 128;
  const bounded = value.every((server) => withinUnicodeCharacterLimit(server, maxAddressLength));
  const servers = value.map(safeTargetPart);
  return {
    text: `${family} ${servers.map((server) => server.text).join(", ")}`,
    complete: bounded && servers.every((server) => server.complete),
  };
}

function revertTargetDetails(change) {
  const revert = change?.revert ?? {};
  switch (revert.t) {
    case "registry": {
      const key = safeTargetPart(revert.key);
      const value = safeTargetPart(revert.value);
      const old = describeRegistrySnapshot(revert.old);
      const keyText = typeof revert.key === "string" ? revert.key : "";
      const normalizedKeyText = keyText.replace(/^HKEY_LOCAL_MACHINE\\/i, "HKLM\\");
      const isNicClassPath = NIC_PNP_PREFIXES.some((prefix) => normalizedKeyText.toLowerCase().startsWith(prefix.toLowerCase()));
      const isNicTarget = isNicClassPath && typeof revert.value === "string" && revert.value.toLowerCase() === "pnpcapabilities";
      const validNicGuid = typeof revert.nic_guid === "string" && NIC_GUID_PATTERN.test(revert.nic_guid);
      const validNicTarget = !isNicClassPath || (isNicTarget && NIC_PNP_KEY_PATTERN.test(normalizedKeyText) && validNicGuid);
      const hasUnexpectedNicGuid = revert.nic_guid !== null && revert.nic_guid !== undefined && !isNicTarget;
      const nicIdentity = isNicClassPath ? `（适配器 GUID ${safeTargetPart(revert.nic_guid).text}）` : "";
      return {
        text: `注册表 ${key.text}\\${value.text}${nicIdentity} → ${old.text}`,
        complete: key.complete && value.complete && old.complete && validNicTarget && !hasUnexpectedNicGuid,
      };
    }
    case "dns": {
      const iface = safeTargetPart(revert.iface);
      const ifaceBounded = withinUnicodeCharacterLimit(revert.iface, 256);
      const old = revert.old;
      if (!old || typeof old !== "object") return { text: `DNS 网卡 ${iface.text} → 原设置未知`, complete: false };
      const ipv4 = describeDnsFamily(old.ipv4, "IPv4");
      const ipv6 = describeDnsFamily(old.ipv6, "IPv6");
      return {
        text: `DNS 网卡 ${iface.text}${ifaceBounded ? "" : "（网卡名超过 256 字符）"} → ${ipv4.text}; ${ipv6.text}`,
        complete: iface.complete && ifaceBounded && ipv4.complete && ipv6.complete,
      };
    }
    case "power_plan": {
      const guid = safeTargetPart(revert.guid);
      const valid = typeof revert.guid === "string" && GUID_PATTERN.test(revert.guid);
      return {
        text: `还原电源计划 ${guid.text}${valid ? "" : "（GUID 无效）"}`,
        complete: guid.complete && valid,
      };
    }
    case "tcp": {
      const level = safeTargetPart(revert.level);
      const valid = typeof revert.level === "string" && TCP_LEVELS.has(revert.level.toLowerCase());
      return {
        text: `TCP 自动调优恢复为 ${level.text}${valid ? "" : "（级别无效）"}`,
        complete: level.complete && valid,
      };
    }
    case "epp": {
      const value = revert.old;
      const valid = Number.isInteger(value) && value >= 0 && value <= 100;
      return {
        text: valid ? `系统能源性能偏好（EPP）恢复为 ${value}` : "系统能源性能偏好（EPP）原值未知或无效",
        complete: valid,
      };
    }
    default: {
      const type = safeTargetPart(revert.t ?? "none");
      return { text: `未知操作目标（${type.text}）`, complete: false };
    }
  }
}

export function getChangeRevertTarget(change) {
  return revertTargetDetails(change).text;
}

export function summarizeUndoTargets(changes, limit = 20) {
  const details = changes.map(revertTargetDetails);
  const targets = details.map((detail, index) => `${index + 1}. ${detail.text}`);
  const invalidTarget = details.some((detail) => !detail.complete);
  const tooMany = targets.length > limit;
  const tooLong = targets.reduce((total, target) => total + target.length + 1, 0) > MAX_CONFIRMATION_CHARS;
  return {
    complete: !tooMany && !tooLong && !invalidTarget,
    tooMany,
    tooLong,
    invalidTarget,
    total: targets.length,
    visible: targets.slice(0, limit),
  };
}

function isReversible(change) {
  return String(change?.revert?.t ?? "none") !== "none";
}

export function filterChanges(changes, query = "", category = "all", reversibility = "all") {
  const needle = String(query).trim().toLocaleLowerCase();
  return changes.filter((change) => {
    const type = String(change?.revert?.t ?? "none");
    const reversible = isReversible(change);
    if (reversibility === "reversible" && !reversible) return false;
    if (reversibility === "info" && reversible) return false;
    if (category !== "all" && (category === "other" ? Object.hasOwn(CATEGORY_LABELS, type) : type !== category)) return false;
    if (!needle) return true;
    const searchable = [change?.desc, change?.group_label, change?.id, getChangeCategory(type)]
      .map((value) => String(value ?? ""))
      .join(" ")
      .toLocaleLowerCase();
    return searchable.includes(needle);
  });
}

function csvCell(value) {
  const text = String(value ?? "");
  const formulaSafe = /^[\s\u0000-\u001f]*[=+@\-]/.test(text) ? `'${text}` : text;
  const clean = formulaSafe.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
  return `"${clean.replace(/"/g, '""')}"`;
}

function timeIso(timestamp) {
  if (typeof timestamp !== "number" || !Number.isFinite(timestamp)) return "";
  const date = new Date(timestamp * 1000);
  return Number.isNaN(date.getTime()) ? "" : date.toISOString();
}

export function buildChangeJournalCsv(changes, appVersion) {
  const rows = [
    ["应用版本", "记录 ID", "时间 UTC", "描述", "分组", "类别", "还原状态", "还原类型"],
    ...changes.map((change) => [
      appVersion,
      change.id,
      timeIso(change.ts),
      change.desc,
      change.group_label ?? "",
      getChangeCategory(change.revert?.t),
      isReversible(change) ? "可还原" : "仅记录",
      change.revert?.t ?? "none",
    ]),
  ];
  return "\uFEFF" + rows.map((row) => row.map(csvCell).join(",")).join("\r\n");
}

export function buildChangeJournalJson(changes, appVersion) {
  const entries = changes.map((change) => ({
    id: change.id,
    timestamp_utc: timeIso(change.ts),
    description: change.desc,
    group: change.group_label ?? null,
    category: getChangeCategory(change.revert?.t),
    reversible: isReversible(change),
    revert_type: change.revert?.t ?? "none",
  }));
  return JSON.stringify({
    app: "AeroTune",
    app_version: appVersion,
    exported_at_utc: new Date().toISOString(),
    count: entries.length,
    changes: entries,
  }, null, 2);
}
