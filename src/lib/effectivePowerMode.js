const EFFECTIVE_POWER_MODE_PRESENTATION = {
  battery_saver: {
    label: "电池节能",
    tone: "amber",
    description: "Windows 当前报告电池节能策略。",
  },
  better_battery: {
    label: "偏向续航",
    tone: "green",
    description: "Windows 当前报告偏向能效与续航。",
  },
  balanced: {
    label: "平衡",
    tone: "cyan",
    description: "Windows 当前报告平衡能效态。",
  },
  high_performance: {
    label: "高性能",
    tone: "rose",
    description: "Windows 当前报告高性能能效态。",
  },
  max_performance: {
    label: "最佳性能",
    tone: "rose",
    description: "Windows 当前报告最高性能能效态。",
  },
  game_mode: {
    label: "游戏模式",
    tone: "violet",
    description: "Windows 当前报告游戏能效态。",
  },
  mixed_reality: {
    label: "混合现实",
    tone: "violet",
    description: "Windows 当前报告混合现实能效态。",
  },
};

const UNKNOWN_PRESENTATION = {
  label: "未知 / 暂不可用",
  tone: "slate",
  description: "当前没有可用的 Windows 有效能效态样本（可能尚未读取成功、系统不支持或值暂不可识别）；不会据此推断当前模式。",
};

const LOADING_PRESENTATION = {
  label: "读取中…",
  tone: "slate",
  description: "正在等待 Windows 有效能效态。",
};

export function effectivePowerModePresentation(mode, loading = false, stale = false) {
  if (loading) return LOADING_PRESENTATION;
  const known = typeof mode === "string" && Object.prototype.hasOwnProperty.call(EFFECTIVE_POWER_MODE_PRESENTATION, mode);
  if (stale) {
    const previous = known ? EFFECTIVE_POWER_MODE_PRESENTATION[mode] : null;
    return {
      label: previous ? `上次观察：${previous.label}` : "上次状态未知",
      tone: "amber",
      description: previous
        ? `最近一次读取时，Windows 报告${previous.label}；实时遥测已陈旧，不能视作当前模式。`
        : "实时遥测已陈旧，Windows 当前有效能效态不可确认。",
    };
  }
  return known ? EFFECTIVE_POWER_MODE_PRESENTATION[mode] : UNKNOWN_PRESENTATION;
}

/** Explain only a notable state pairing; never imply one setting caused the other. */
export function effectivePowerModeContext(mode, aeroTunePowerEnabled) {
  if (aeroTunePowerEnabled == null) return null;
  if (mode === "battery_saver" && !aeroTunePowerEnabled) {
    return "Windows 仍报告电池节能态，而 AeroTune 省电预设已关闭。Windows 状态独立于 AeroTune 开关；如非预期，可核对 Windows 电源设置。";
  }
  if (aeroTunePowerEnabled && (mode === "high_performance" || mode === "max_performance")) {
    const label = EFFECTIVE_POWER_MODE_PRESENTATION[mode].label;
    return `AeroTune 省电预设已开启，但 Windows 报告的是${label}。两者是独立策略信号；如体验与预期不符，可核对 Windows 活动电源配置。`;
  }
  return null;
}
