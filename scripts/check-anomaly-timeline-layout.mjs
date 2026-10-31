import assert from "node:assert/strict";
import { anomalyTimelineLayout, formatAnomalyOffset } from "../src/lib/anomalyTimeline.js";

assert.equal(formatAnomalyOffset(990, 1000), "−10秒");
assert.equal(formatAnomalyOffset(1000, 1000), "起点");
assert.equal(formatAnomalyOffset(1010, 1000), "+10秒");

const onset = 1000;
const layout = anomalyTimelineLayout([{ at_unix: 1000 }, { at_unix: 700 }, { at_unix: 1010 }], onset);
assert.ok(layout.min < 700 && layout.max > 1010, "all events have visible margins");
assert.ok(layout.ticks.some((tick) => tick.time < onset && tick.label.startsWith("−")));
assert.ok(layout.ticks.every((tick) => tick.label === "起点" || tick.label.endsWith("秒")));
const after = anomalyTimelineLayout([{ at_unix: 1000 }, { at_unix: 1100 }], onset);
assert.ok(after.max > 1100, "events after the onset remain within the chart");
assert.ok(after.ticks.some((tick) => tick.time > onset && tick.label.startsWith("+")), "post-onset ticks must not claim to precede onset");
const short = anomalyTimelineLayout([{ at_unix: 1000 }], onset);
assert.ok(short.span >= 60);
console.log("PASS anomaly timeline bounds and signed onset-relative tick labels");
