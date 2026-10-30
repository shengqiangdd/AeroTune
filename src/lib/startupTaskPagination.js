export const STARTUP_TASK_PAGE_SIZE = 50;

export function paginateStartupTasks(items, page, pageSize = STARTUP_TASK_PAGE_SIZE) {
  const source = Array.isArray(items) ? items : [];
  const boundedPageSize = Number.isInteger(pageSize) && pageSize > 0 ? pageSize : STARTUP_TASK_PAGE_SIZE;
  const pageCount = Math.max(1, Math.ceil(source.length / boundedPageSize));
  const currentPage = Number.isInteger(page) ? Math.max(0, Math.min(page, pageCount - 1)) : 0;
  const start = currentPage * boundedPageSize;

  return {
    page: currentPage,
    pageCount,
    items: source.slice(start, start + boundedPageSize),
  };
}

export function filterStartupTasks(tasks, query) {
  const source = Array.isArray(tasks) ? tasks : [];
  const normalizedQuery = typeof query === "string" ? query.trim().toLocaleLowerCase() : "";
  if (!normalizedQuery) return source;

  const terms = normalizedQuery.split(/\s+/);
  return source.filter((task) => {
    const searchable = `${task?.name ?? ""} ${task?.path ?? ""} ${task?.action ?? ""} ${task?.author ?? ""}`.toLocaleLowerCase();
    return terms.every((term) => searchable.includes(term));
  });
}
