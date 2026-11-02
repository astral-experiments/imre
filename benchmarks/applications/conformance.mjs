// The GitHub report mode of examples/conformance_report, on the same frozen inputs.
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { parseArgs } from "node:util";

const { values: args } = parseArgs({
  options: {
    manifest: { type: "string" },
    old: { type: "string" },
    new: { type: "string" },
    output: { type: "string" },
    format: { type: "string" },
    "force-summary-table": { type: "boolean" },
  },
});
if (args.format !== "github" || !args["force-summary-table"])
  throw new Error("expected GitHub summary mode");
const read = (path) => JSON.parse(readFileSync(path, "utf8"));
const manifest = read(args.manifest);
const baseURL = `https://github.com/python/typing/blob/${manifest.suite_revision}/conformance/`;
const aliases = new Map(
  manifest.diagnostic_paths.map((alias) => [alias.raw, alias.path]),
);
const cases = new Map();
const tags = new Map();
const files = new Map();
const empty = () => ({ tp: 0, fp: 0, fn: 0, total: 0 });
const passes = (s) => s.fp === 0 && s.fn === 0;
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const signed = (n) => (n >= 0 ? "+" + n : String(n));
const pct = (n) => (100 * n).toFixed(2) + "%";

function getCase(path, key) {
  if (!cases.has(key))
    cases.set(key, { path, key, expected: [], old: [], new: [] });
  return cases.get(key);
}
for (const path of manifest.test_files) {
  files.set(path, { path, old: empty(), new: empty() });
  const lines = readFileSync(path, "utf8").split(
    /\r\n|[\n\r\v\f\x1c-\x1e\x85\u2028\u2029]/,
  );
  for (const [i, line] of lines.entries()) {
    const m = /#\s*E(\?)?(?:\[([^+\]]+)(\+)?\])?(?:\s*:\s*(.*))?/.exec(line);
    if (!m) continue;
    const prefix = line.slice(0, m.index).trimStart();
    if (!prefix || prefix.startsWith("#")) continue;
    const position = path + ":" + (i + 1);
    const key = m[2] ? path + ":" + basename(path) + ":" + m[2] : position;
    if (m[2]) tags.set(position, key);
    getCase(path, key).expected.push({
      optional: Boolean(m[1]),
      multi: Boolean(m[3]),
    });
  }
}
for (const side of ["old", "new"]) {
  for (const d of read(args[side])) {
    if (d.severity !== "major") continue;
    const path = aliases.get(d.location.path);
    if (!path) throw new Error("unknown diagnostic path");
    const { line, column } = d.location.positions.begin;
    const pos = path + ":" + line;
    getCase(path, tags.get(pos) ?? pos)[side].push({
      rule: d.check_name,
      text: d.description.replaceAll(d.check_name + ": ", ""),
      line,
      column,
    });
  }
}

function classification(tc, side, optional) {
  if (tc[side].length === 0)
    return tc.expected.length && !optional ? "fn" : "tn";
  if (optional) return "tp";
  if (!tc.expected.length) return "fp";
  const max = tc.expected.every((e) => e.multi) ? tc.expected.length : 1;
  return new Set(tc[side].map((d) => d.line)).size <= max ? "tp" : "fp";
}
const titles = [
  "True positives added",
  "False positives removed",
  "True positives changed",
  "False positives changed",
  "False positives added",
  "True positives removed",
  "Optional Diagnostics Added",
  "Optional Diagnostics Removed",
  "Optional Diagnostics Changed",
];
const section = new Map();
const names = {
  tp: "True positives",
  fp: "False positives",
  tn: "True negatives",
  fn: "False negatives",
};
for (const tc of [...cases.values()].sort((a, b) => cmp(a.key, b.key))) {
  const optional =
    tc.expected.length > 0 && tc.expected.every((e) => e.optional);
  const kinds = {};
  for (const side of ["old", "new"]) {
    const kind = classification(tc, side, optional);
    kinds[side] = kind;
    const s = files.get(tc.path)[side];
    if (kind !== "tn") s[kind]++;
    s.total += tc[side].length;
  }
  function add(verb, side) {
    const title = optional
      ? "Optional Diagnostics " + verb[0].toUpperCase() + verb.slice(1)
      : names[kinds[side ?? "new"]] + " " + verb;
    if (!section.has(title)) section.set(title, []);
    section.get(title).push({ tc, side });
  }
  if (!tc.old.length && tc.new.length) add("added", "new");
  else if (tc.old.length && !tc.new.length) add("removed", "old");
  else if (tc.old.length) {
    const fingerprint = (ds) =>
      JSON.stringify(ds.map((d) => JSON.stringify(d)).sort());
    if (fingerprint(tc.old) !== fingerprint(tc.new)) {
      if (optional || kinds.old === kinds.new) add("changed", null);
      else {
        add("removed", "old");
        add("added", "new");
      }
    }
  }
}

const totals = { old: empty(), new: empty() };
const passing = { old: 0, new: 0 };
for (const file of files.values()) {
  for (const side of ["old", "new"]) {
    for (const key of Object.keys(totals[side]))
      totals[side][key] += file[side][key];
    passing[side] += Number(passes(file[side]));
  }
}
const { old, new: next } = totals;
const precision = (s) => s.tp / (s.tp + s.fp) || 0;
const recall = (s) => s.tp / (s.tp + s.fn) || 0;
const po = precision(old),
  pn = precision(next),
  ro = recall(old),
  rn = recall(next);
const pf = (n) => n + "/" + files.size;
function metric(a, b, format, up, down) {
  if (a === b) return "held steady at <b>" + format(a) + "</b>";
  return (
    (b > a ? up : down) +
    " from <b>" +
    format(a) +
    "</b> to <b>" +
    format(b) +
    "</b>"
  );
}
const progress = [pn - po, rn - ro, passing.new - passing.old];
let heading = `[Typing conformance results](${baseURL})`;
if (progress.some((x) => x > 0) && progress.every((x) => x >= 0))
  heading += " improved 🎉";
if (progress.some((x) => x < 0) && progress.every((x) => x <= 0))
  heading += " regressed ❌";
const paragraph =
  "The percentage of diagnostics emitted that were expected errors " +
  metric(po, pn, pct, "increased", "decreased") +
  ". The percentage of expected errors that received a diagnostic " +
  metric(ro, rn, pct, "increased", "decreased") +
  ". The number of fully passing files " +
  metric(passing.old, passing.new, pf, "improved", "regressed") +
  ".";
const note =
  "Each test case represents one expected error annotation or a group of annotations sharing a tag. Counts are per test case, not per diagnostic — multiple diagnostics on the same line count as one. Required annotations (`E`) are true positives when ty flags the expected location and false negatives when it does not. Optional annotations (`E?`) are true positives when flagged but true negatives (not false negatives) when not. Tagged annotations (`E[tag]`) require ty to flag exactly one of the tagged lines; tagged multi-annotations (`E[tag+]`) allow any number up to the tag count. Flagging unexpected locations counts as a false positive.";
const summary = [
  "",
  "## " + heading,
  "",
  paragraph,
  "",
  "### Summary",
  "",
  "<details>",
  "<summary>How are test cases classified?</summary>",
  "",
  "<br>",
  "",
  note,
  "",
  "</details>",
  "",
  "| Metric | Old | New | Diff | Outcome |",
  "|--------|-----|-----|------|---------|",
];
for (const [title, a, b, fmt, good] of [
  ["True Positives ", old.tp, next.tp, String, 1],
  ["False Positives", old.fp, next.fp, String, -1],
  ["False Negatives", old.fn, next.fn, String, -1],
  ["Total Diagnostics", old.total, next.total, String, 0],
  ["Precision", po, pn, pct, 1],
  ["Recall", ro, rn, pct, 1],
  ["Passing Files", passing.old, passing.new, pf, 1],
]) {
  const diff = b - a;
  const shown = fmt === pct ? (diff >= 0 ? "+" : "") + pct(diff) : signed(diff);
  const outcome =
    diff === 0
      ? ""
      : (diff > 0 ? "⏫" : "⏬") +
        (good ? (good * diff > 0 ? " (✅)" : " (❌)") : "");
  summary.push(`| ${title} | ${fmt(a)} | ${fmt(b)} | ${shown} | ${outcome} |`);
}
const sections = [summary.join("\n") + "\n\n"];
const delta = (f) =>
  ["tp", "fp", "fn"].reduce((n, k) => n + Math.abs(f.new[k] - f.old[k]), 0);
const changed = [...files.values()]
  .filter((f) => delta(f))
  .sort(
    (a, b) => delta(b) - delta(a) || cmp(basename(a.path), basename(b.path)),
  );
function counts(a, b) {
  return ["tp", "fp", "fn"].map((k) => {
    const d = b[k] - a[k];
    return d
      ? `${b[k]} (${signed(d)}) ${(k === "tp" ? d > 0 : d < 0) ? "✅" : "❌"}`
      : String(b[k]);
  });
}
if (changed.length) {
  const lines = [
    "### Test file breakdown",
    "",
    "<details>",
    `<summary>${changed.length} file${changed.length === 1 ? "" : "s"} altered</summary>`,
    "",
    "| File | True Positives | False Positives | False Negatives | Status |",
    "|------|----|----|----|--------|",
  ];
  for (const f of changed) {
    const name = basename(f.path);
    const d = f.new.fp + f.new.fn - f.old.fp - f.old.fn;
    const status = passes(f.new)
      ? passes(f.old)
        ? "✅ Still Passing"
        : "✅ Newly Passing 🎉"
      : passes(f.old)
        ? "❌ Newly Failing ☹️"
        : d < 0
          ? "📈 Improving"
          : d > 0
            ? "📉 Regressing"
            : "➡️ Neutral";
    lines.push(
      `| [${name}](${baseURL}tests/${name}) | ${counts(f.old, f.new).join(" | ")} | ${status} |`,
    );
  }
  lines.push(
    `| **Total (all files)** | ${counts(old, next)
      .map((x) => "**" + x + "**")
      .join(" | ")} | ${pf(passing.new)} |`,
    "",
    "</details>",
    "",
  );
  sections.push(lines.join("\n"));
}
if (section.size) {
  const lines = [];
  const rank = (s) => (titles.includes(s) ? titles.indexOf(s) : 99);
  for (const title of [...section.keys()].sort(
    (a, b) => rank(a) - rank(b) || cmp(a, b),
  )) {
    const entries = section.get(title);
    lines.push(
      `### ${title} (${entries.length})`,
      "",
      "<details>",
      `<summary>${entries.length} diagnostic${entries.length === 1 ? "" : "s"}</summary>`,
      "",
      "<table>",
      "<tr>",
      "<th>Test case</th>",
      "<th>Diff</th>",
      "</tr>",
    );
    for (const { tc, side } of entries) {
      const sides = side ? [side] : ["old", "new"];
      const numbers = sides.flatMap((s) => tc[s].map((d) => d.line));
      const lo = Math.min(...numbers),
        hi = Math.max(...numbers),
        file = basename(tc.path);
      const link = `[${file}:${lo}${hi === lo ? "" : ":" + hi}](${baseURL}tests/${file}#L${lo}${hi === lo ? "" : "-L" + hi})`;
      lines.push(
        "",
        "<tr>",
        "<td>",
        "",
        link,
        "",
        "</td>",
        "",
        "<td>",
        "",
        "```diff",
      );
      for (const s of sides)
        for (const d of tc[s])
          lines.push(
            (s === "old" ? "-" : "+") + "error[" + d.rule + "] " + d.text,
          );
      lines.push("```", "", "</td>", "</tr>");
    }
    lines.push("</table>", "</details>", "");
  }
  sections.push(lines.join("\n"));
}
writeFileSync(args.output, sections.join("\n\n"));
