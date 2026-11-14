import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const [pluginPath, tarballPath] = process.argv.slice(2);
if (!pluginPath) {
  console.error("usage: node tools/timeago/generate.mjs <jquery.timeago.js> [<timeago-1.6.7.tgz>]");
  process.exit(2);
}

const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

const jquery = {
  extend(target, ...sources) {
    return Object.assign(target, ...sources);
  },
  fn: { extend() {} },
  expr: { ":": {} },
  isFunction: (value) => typeof value === "function",
  trim: (value) => String(value).trim(),
  contains: () => true,
};
const context = vm.createContext({
  jQuery: jquery,
  document: { createElement() {}, documentElement: {} },
  window: {},
});
vm.runInContext(readFileSync(pluginPath, "utf8"), context, { filename: "jquery.timeago.js" });

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;
const YEAR = 365 * DAY;

const boundaries = new Set([45, 90, 45 * MINUTE, 90 * MINUTE, 24 * HOUR, 42 * HOUR, 30 * DAY, 45 * DAY, YEAR, 1.5 * YEAR]);
for (let k = 0; k <= 45; k++) boundaries.add((k + 0.5) * MINUTE);
for (let k = 1; k <= 24; k++) boundaries.add((k + 0.5) * HOUR);
for (let k = 1; k <= 30; k++) boundaries.add((k + 0.5) * DAY);
for (let k = 1; k <= 12; k++) boundaries.add((k + 0.5) * 30 * DAY);
for (let k = 1; k <= 40; k++) boundaries.add((k + 0.5) * YEAR);

const seconds = new Set([0, 1, 30, 60, 100, 3600, 86400, 40 * DAY, 800 * DAY]);
for (const boundary of boundaries) {
  for (const offset of [-1, -0.5, 0, 0.5, 1]) {
    const value = boundary + offset;
    if (value >= 0) seconds.add(value);
  }
}

const cases = [...seconds]
  .sort((a, b) => a - b)
  .map((value) => [value, context.jQuery.timeago.inWords(value * 1000)]);

const source = { package: "timeago", version: "1.6.7", plugin_sha256: sha256(pluginPath) };
if (tarballPath) source.tarball_sha256 = sha256(tarballPath);

const output = join(dirname(fileURLToPath(import.meta.url)), "../../crates/lost-scores-app/tests/data/timeago-1.6.7.json");
writeFileSync(output, `${JSON.stringify({ source, cases })}\n`);
console.log(`wrote ${cases.length} cases to ${output}`);
