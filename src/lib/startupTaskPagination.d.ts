export const STARTUP_TASK_PAGE_SIZE: 50;

export interface StartupTaskPage<T> {
  page: number;
  pageCount: number;
  items: T[];
}

export function paginateStartupTasks<T>(
  items: readonly T[] | null | undefined,
  page: number,
  pageSize?: number,
): StartupTaskPage<T>;

export interface StartupTaskSearchable {
  name?: string;
  path?: string;
  action?: string;
  author?: string;
}

export function filterStartupTasks<T extends StartupTaskSearchable>(
  tasks: readonly T[] | null | undefined,
  query: string | null | undefined,
): T[];
