export type LocalObservationHistoryId = "network" | "disk" | "process" | "battery" | "experiments";

export type LocalObservationHistoryTarget = {
  id: LocalObservationHistoryId;
  label: string;
  detail: string;
  storage: "localStorage";
  key: string;
};

export type LocalObservationHistoryInventoryItem = {
  id: LocalObservationHistoryId;
  label: string;
  state: "present" | "absent" | "unknown";
};

type LocalObservationHistoryStorage = Partial<Pick<Storage, "removeItem" | "key" | "length">>;
type LocalObservationHistoryStorageOverrides = { localStorage?: LocalObservationHistoryStorage | null };

export type LocalObservationHistoryResult = {
  id: LocalObservationHistoryId;
  label: string;
  ok: boolean;
};

export const LOCAL_OBSERVATION_HISTORY_TARGETS: readonly LocalObservationHistoryTarget[];

export function inspectLocalObservationHistory(
  storageOverrides?: LocalObservationHistoryStorageOverrides,
): LocalObservationHistoryInventoryItem[];

export function clearLocalObservationHistory(
  ids: readonly LocalObservationHistoryId[],
  storageOverrides?: LocalObservationHistoryStorageOverrides,
): LocalObservationHistoryResult[];
