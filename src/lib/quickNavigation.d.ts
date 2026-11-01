export const RECENT_NAVIGATION_KEY: string;
export const MAX_RECENT_PAGES: number;
export const NAVIGATION_SEARCH_TERMS: Readonly<Record<string, readonly string[]>>;

export interface SettingsDeepLink {
  id: string;
  label: string;
  group: string;
  routeId: "settings";
  anchor: string;
  keywords: readonly string[];
}
export const SETTINGS_DEEP_LINKS: readonly SettingsDeepLink[];

export function readRecentPageIds(validIds: readonly string[], storage?: Pick<Storage, "getItem"> | null): string[];
export function nextRecentPageIds(pageId: string, recentIds: readonly string[], validIds: readonly string[]): string[];
export function persistRecentPageIds(pageIds: readonly string[], validIds: readonly string[], storage?: Pick<Storage, "setItem"> | null): void;
export function clearRecentPageIds(storage?: Pick<Storage, "removeItem" | "setItem"> | null): boolean;
export function shouldSelectQuickNavigationResult(activeElement: Element | null, inputElement: Element | null, hasSelection: boolean): boolean;
export function getTabFocusTarget<T>(focusables: readonly T[], activeElement: unknown, shiftKey: boolean, focusInsideDialog: boolean): T | null;
export function rankNavigationItems<T extends { id: string; label: string; group: string }>(
  items: readonly T[],
  query: string,
  recentIds?: readonly string[],
  deepLinks?: readonly SettingsDeepLink[],
): Array<(T & { quickGroup: string }) | (SettingsDeepLink & { quickGroup: string })>;
