import assert from "node:assert/strict";
import { matchStartupImpactPathToItem } from "../src/lib/startupImpactMatch.js";

const item = (id, name, cmd, source = "Run") => ({ id, name, cmd, source, enabled: true });
const quoted = item("quoted", "Widget", '"C:\\Program Files\\Vendor\\Widget.exe" --silent /minimized');
const simple = item("simple", "Agent", "C:\\Tools\\Agent.exe --profile default");

assert.equal(matchStartupImpactPathToItem("c:/program files/VENDOR/widget.EXE", [quoted]).kind, "exact", "quoted executable paths match despite slash and case differences");
assert.equal(matchStartupImpactPathToItem("C:/TOOLS/AGENT.exe", [simple]).kind, "exact", "unquoted no-space executable paths ignore arguments");

const sameBasenameA = item("dir-a", "Worker A", '"C:\\VendorA\\worker.exe"');
const sameBasenameB = item("dir-b", "Worker B", '"D:\\VendorB\\worker.exe"');
const basenameResult = matchStartupImpactPathToItem("d:/vendorb/WORKER.exe", [sameBasenameA, sameBasenameB]);
assert.equal(basenameResult.kind, "exact", "a duplicate basename in another directory does not prevent an exact path match");
if (basenameResult.kind === "exact") assert.equal(basenameResult.item.id, "dir-b");

const duplicatePath = matchStartupImpactPathToItem("C:\\Tools\\Agent.exe", [simple, item("simple-copy", "Agent copy", '"c:/tools/agent.EXE" --other')]);
assert.equal(duplicatePath.kind, "ambiguous", "duplicate entries for the same normalized executable path are not arbitrarily selected");
if (duplicatePath.kind === "ambiguous") assert.equal(duplicatePath.items.length, 2);

assert.equal(matchStartupImpactPathToItem(undefined, [simple]).kind, "none", "missing impact paths use the name-search fallback");
assert.equal(matchStartupImpactPathToItem("Agent.exe", [simple]).kind, "none", "relative impact paths never match by basename");
assert.equal(matchStartupImpactPathToItem("%ProgramFiles%\\Vendor\\Widget.exe", [quoted]).kind, "none", "unexpanded environment-variable paths are not guessed");
assert.equal(matchStartupImpactPathToItem("C:\\Program Files\\Vendor\\Widget.exe", [item("unquoted", "Widget", "C:\\Program Files\\Vendor\\Widget.exe")]).kind, "none", "unquoted spaced command paths are treated as ambiguous");
assert.equal(matchStartupImpactPathToItem("C:\\Other\\missing.exe", [simple]).kind, "none", "valid but unmatched paths use the name-search fallback");

console.log("Startup impact path-match regression passed: quotes, args, normalization, duplicate basenames, ambiguity, and fallbacks");

