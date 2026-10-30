import type { StartupItem } from "./api";

export function startupTrustLabel(
  status: StartupItem["signature_status"] | string,
  signerSubject?: string | null,
): string;
