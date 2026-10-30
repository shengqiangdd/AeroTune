import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const processes = read("src/pages/Processes.tsx");
const dashboard = read("src/pages/Dashboard.tsx");
const api = read("src/lib/api.ts");

assert.match(
  processes,
  /<button\s+type="button"\s+disabled\s+aria-label=\{`挂起进程暂不可用，\$\{p\.name\}，PID \$\{p\.pid\}`\}/s,
  "new process suspension must be visibly and accessibly disabled",
);
assert.doesNotMatch(processes, /api\.suspendProcess\(/, "the UI must not invoke the disabled suspension IPC");
assert.match(processes, /新挂起操作暂时关闭[\s\S]*恢复入口仍保留/, "suspension shutdown and recovery status must be visible");

assert.match(
  processes,
  /disabled=\{rowMutationsDisabled\}[\s\S]*?aria-label=\{rowMutationsDisabled[\s\S]*?\$\{rowMutationDisabledReason\}[\s\S]*?创建时间 \$\{creationTime\}/,
  "resume must be identity-bound and accessibly disabled if identity is unavailable",
);
assert.match(processes, /const rowMutationDisabledReason = processMutationsDisabled \? "进程列表需刷新" : "无法核验创建时间，请刷新进程列表"/);
assert.match(processes, /const requireVerifiedActionIdentity = \(p: ProcInfo\)[\s\S]*?const creationToken = p\.creation_token[\s\S]*?const creationTime = formatWindowsFiletimeUtc\(creationToken\)[\s\S]*?if \(!creationToken \|\| !creationTime\)[\s\S]*?return null/);
assert.match(processes, /const resume = async \(p: ProcInfo\)[\s\S]*?requireVerifiedActionIdentity\(p\)[\s\S]*?api\.resumeProcess\(p\.pid, identity\.creationToken\)/);
assert.match(processes, /创建时间 \$\{identity\.creationTime\}[\s\S]*?后端会用列表捕获的创建时间令牌核对当前进程/);
assert.ok(processes.includes("onClick={() => resume(p)}"), "resume must remain available for verified identities");
assert.ok(processes.includes('<Th k="pid" className="w-48" title="按 PID 排序；启动时间用于区分进程实例">PID / 启动时间 (UTC)</Th>'), "process start time must be visible in a dedicated UTC column, with PID sort semantics clear");
assert.ok(processes.includes('const visibleCreationTime = creationTime ? `${creationTime.slice(5, 27)} UTC` : "无法核验";'), "the visible column must show the captured UTC start time or an explicit unknown state");
assert.ok(processes.includes("{visibleCreationTime}"), "the verified start time must be rendered in the process row");
assert.match(api, /resumeProcess:\s*\(pid: number, creationToken: string\)[\s\S]*?invoke<void>\("resume_process", \{ pid, creationToken \}\)/);

assert.match(
  processes,
  /aria-label=\{rowMutationsDisabled[\s\S]*?`结束进程暂不可用，\$\{p\.name\}，PID \$\{p\.pid\}，\$\{rowMutationDisabledReason\}`[\s\S]*?`结束进程 \$\{p\.name\}，PID \$\{p\.pid\}`/,
  "terminate must describe both disabled-stale and operable action states",
);
assert.ok(processes.includes("onClick={() => kill(p)}"), "terminate action must remain operable when snapshot is fresh");

assert.match(
  dashboard,
  /<button\s+type="button"\s+aria-label=\{`查看进程 \$\{a\.name\}，PID \$\{a\.pid\}，前往列表核验当前实例`\}\s+className=/s,
  "dashboard anomaly action must offer an accessible route to verify the current process instance",
);
assert.match(dashboard, /onClick=\{\(\) => onNav\("processes", a\.pid\)\}/);
assert.doesNotMatch(dashboard, /api\.killProcess\(/, "dashboard anomalies must not terminate by an unverified PID");

console.log("PASS process actions are accessible; dashboard anomalies route to identity verification");
