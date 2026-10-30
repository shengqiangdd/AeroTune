import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { rankSymptomFocus, symptomForRecommendation } from "../src/lib/symptomFocus.js";

const backendRecommendations = await readFile(new URL("../src-tauri/src/recommendations.rs", import.meta.url), "utf8");
const fixedBackendIds = [...backendRecommendations.matchAll(/suggestion\(\s*"([^"]+)"/g)].map((match) => match[1]);
assert.ok(fixedBackendIds.length > 0, "backend source should expose fixed recommendation IDs");
for (const id of fixedBackendIds) assert.ok(symptomForRecommendation(id), `backend recommendation ${id} must map to a focused symptom`);

assert.equal(symptomForRecommendation("process-101-cpu"), "lag");
assert.equal(symptomForRecommendation("process-102-mem"), "memory");
assert.equal(symptomForRecommendation("gpu-temperature-high"), "gpu");
assert.equal(symptomForRecommendation("gpu-memory-pressure"), "gpu");
assert.equal(symptomForRecommendation("process-103-io"), "io");
assert.equal(symptomForRecommendation("pagefile-pressure"), "memory");
assert.equal(symptomForRecommendation("startup-burden"), "startup");
assert.equal(symptomForRecommendation("battery-low"), "battery");
assert.equal(symptomForRecommendation("system-disk-low"), "disk");
assert.equal(symptomForRecommendation("unknown-action"), null);

const first = { id: "process-101-cpu", title: "CPU 高占用", reason: "持续占用", action: "processes", priority: 75 };
const strongest = { id: "memory-pressure", title: "内存压力", reason: "余量低", action: "processes", priority: 90 };
const tied = { id: "battery-low", title: "电池偏低", reason: "电量低", action: "power", priority: 75 };
const invalid = { id: "startup-burden", title: "启动项", reason: "bad score", action: "startup", priority: Number.NaN };
const gpuOnly = { id: "gpu-temperature-high", title: "显卡温度", reason: "温度偏高", action: "processes", priority: 82 };
const ioOnly = { id: "process-103-io", title: "异常磁盘吞吐", reason: "高于基线", action: "processes", priority: 70 };
assert.deepEqual(rankSymptomFocus([gpuOnly]), { symptom: "gpu", recommendation: gpuOnly });
assert.deepEqual(rankSymptomFocus([ioOnly]), { symptom: "io", recommendation: ioOnly });
assert.deepEqual(rankSymptomFocus([first, gpuOnly, ioOnly]), { symptom: "gpu", recommendation: gpuOnly });
assert.deepEqual(rankSymptomFocus([first, strongest, tied]), { symptom: "memory", recommendation: strongest });
assert.deepEqual(rankSymptomFocus([first, tied]), { symptom: "lag", recommendation: first });
assert.equal(rankSymptomFocus([invalid, { ...first, id: "unmapped" }]), null);
assert.equal(rankSymptomFocus([]), null);

console.log("Symptom focus passed: GPU/I/O mapping, highest priority, stable ties, and empty/invalid results");
