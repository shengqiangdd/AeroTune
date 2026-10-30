export type LocalObservationHistoryId = "network" | "disk" | "process" | "battery" | "experiments";

export type LocalObservationHistoryTarget = {
  id: LocalObservationHistoryId;
  label: string;
  storage: "localStorage";
  key: string;
};

export type LocalObservationHistoryResult = {
  id: LocalObservationHistoryId;
  label: string;
  ok: boolean;
};

export const LOCAL_OBSERVATION_HISTORY_TARGETS: readonly LocalObservationHistoryTarget[];

export function clearLocalObservationHistory(
  ids?: readonly LocalObservationHistoryId[],
  storageOverrides?: {
    localStorage?: Pick<Storage, "removeItem"> | null;
  },
): LocalObservationHistoryResult[];
