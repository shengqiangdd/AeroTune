export function startupTrustLabel(status, signerSubject) {
  if (status === "signed") {
    return signerSubject
      ? `内嵌签名有效 · ${signerSubject}`
      : "内嵌签名有效 · 未读取到证书显示名";
  }
  if (status === "unsigned") {
    return "无内嵌签名 · 目录签名未检查";
  }
  return "未验证 · 不代表安全";
}
