"""Validate compatibility receipts and render both public scorecard views."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
COHORT = json.loads((HERE / "cohort.json").read_text())
SCHEMA = "imre.compatibility.v1"
OUTCOMES = {"pass", "fail", "error", "skip", "timeout", "not_run", "omitted"}
TIERS = ("stock", "O0", "O3")
DIMENSIONS = ("exit_code", "stdout", "stderr", "effects")
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
START = "<!-- compatibility-scorecard:start -->"
END = "<!-- compatibility-scorecard:end -->"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def families():
    with (ROOT / COHORT["historical_families"]).open(newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    result = {r["family"]: {"originals": int(r["originals"]), "historical": int(r["paired"])} for r in rows}
    require(len(result) == len(rows), "duplicate historical family")
    for r in COHORT["supplemental_families"]:
        require(r["family"] not in result, "duplicate supplemental family")
        result[r["family"]] = {"originals": r["originals"], "historical": None}
    return result


def validate_record(r, kind, family_names):
    label = r.get("id", "<missing id>")
    def check(c, msg): require(c, f"{label}: {msg}")
    check(r.get("schema") == SCHEMA and r.get("kind") == kind, "wrong schema or kind")
    check(isinstance(label, str) and label != "<missing id>" and bool(label), "missing id")
    check(r.get("imre_revision") == COHORT["imre_revision"], "wrong Imre revision")
    check(isinstance(r.get("platform"), dict) and all(r["platform"].get(k) for k in ("os", "arch")), "missing platform")
    check(all(r["platform"].get(k) == v for k, v in COHORT["platform"].items()), "platform differs from pinned cohort")
    source = r.get("source", {})
    check(all(isinstance(source.get(k), str) and source[k] for k in ("repo", "commit", "path", "sha256")), "incomplete source")
    check(bool(HEX40.fullmatch(source["commit"])) and bool(HEX64.fullmatch(source["sha256"])), "invalid source digest")
    if kind == "stdlib":
        check(r.get("family") in family_names, "unknown family")
        check(isinstance(r.get("occurrence"), int) and r["occurrence"] >= 1, "invalid occurrence")
        check(source["repo"] == "https://github.com/python/cpython" and source["commit"] == COHORT["cpython_commit"], "wrong CPython source")
        check(bool(HEX40.fullmatch(source.get("git_blob", ""))), "missing upstream Git blob")
    else:
        invocation = r.get("normalized_invocation", {})
        check(isinstance(invocation.get("argv"), list) and invocation["argv"] and isinstance(invocation.get("cwd"), str) and isinstance(invocation.get("env"), dict), "missing normalized invocation")
    runs = r.get("runs", {})
    check(set(runs) == set(TIERS), "must record stock, O0 and O3")
    for tier in TIERS:
        run = runs[tier]
        check(run.get("outcome") in OUTCOMES, f"invalid {tier} outcome")
        check(isinstance(run.get("argv"), list), f"missing {tier} argv")
        check(bool(run.get("evidence")) if run["outcome"] not in {"not_run", "omitted"} else bool(run.get("reason")), f"missing {tier} evidence/reason")
        if tier == "stock" and run["outcome"] != "not_run":
            check(run.get("version") == COHORT["cpython_version"], "stock must be exact CPython 3.14.5")
        if kind == "script" and run["outcome"] == "pass":
            check(isinstance(run.get("exit_code"), int) and all(HEX64.fullmatch(run.get(k, "")) for k in ("stdout_sha256", "stderr_sha256", "effects_sha256")), f"missing {tier} exact observations")
            check(bool(run.get("effects_manifest")), f"missing {tier} effects manifest")
    omission = r.get("omission_reason")
    check("omission_reason" in r and (omission is None or isinstance(omission, str) and bool(omission)), "invalid omission reason")
    check(not any(x["outcome"] == "omitted" for x in runs.values()) or bool(omission), "omission requires reason")
    paired = not omission and all(runs[t]["outcome"] == "pass" for t in TIERS)
    if kind == "script":
        comparisons = r.get("comparisons", {})
        check(set(comparisons) == {"O0", "O3"}, "missing comparisons")
        for tier in ("O0", "O3"):
            comparison = comparisons[tier]
            check(set(comparison) == set(DIMENSIONS) and all(v in {"exact", "different", "not_checked"} for v in comparison.values()), f"invalid {tier} comparison")
            for dim in DIMENSIONS:
                status = comparison[dim]
                field = dim if dim == "exit_code" else f"{dim}_sha256"
                a, b = runs["stock"].get(field), runs[tier].get(field)
                if status != "not_checked":
                    check(a is not None and b is not None and ((a == b) == (status == "exact")), f"{tier} {dim} comparison contradicts observations")
            paired = paired and all(v == "exact" for v in comparison.values())
    check(isinstance(r.get("paired"), bool) and r["paired"] == bool(paired), "paired does not match stock/O0/O3 intersection")
    return r


def verify_stdlib_evidence(r):
    source = r["source"]
    vendored = ROOT / "benchmarks/cpython_stdlib/full" / r["family"] / "upstream" / source["path"]
    if vendored.is_file():
        data = vendored.read_bytes()
        require(hashlib.sha256(data).hexdigest() == source["sha256"] and hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() == source["git_blob"], f"{r['id']}: source hash differs from vendored upstream")
    suffix = ".".join(r["id"].split(".")[-2:])
    statuses = {"pass": ("ok",), "fail": ("FAIL", "expected failure", "unexpected success"), "error": ("ERROR",), "skip": ("skipped ",)}
    for tier, run in r["runs"].items():
        if run["outcome"] not in statuses: continue
        evidence = run["evidence"]
        match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", evidence)
        require(bool(match), f"{r['id']}: {tier} needs a raw evidence line")
        path = (ROOT / match.group(1)).resolve()
        require(path.is_relative_to(ROOT.resolve()) and path.is_file(), f"{r['id']}: missing evidence {evidence}")
        lines = path.read_text().splitlines(); line = int(match.group(2))
        require(line <= len(lines) and suffix in lines[line-1] and any(f" ... {status}" in lines[line-1] for status in statuses[run["outcome"]]), f"{r['id']}: {tier} evidence does not establish outcome")


def load_records(kind, names):
    records = []
    seen = set()
    commands = set()
    for path in sorted((HERE / ("stdlib" if kind == "stdlib" else "scripts")).glob("*.jsonl")):
        if kind == "script": require(path.stem in COHORT["scripts"]["shards"], f"unrecognized script shard: {path}")
        for line_num, line in enumerate(path.read_text().splitlines(), 1):
            try: r = validate_record(json.loads(line), kind, names)
            except (ValueError, TypeError, KeyError) as e: raise ValueError(f"{path.relative_to(ROOT)}:{line_num}: {e}") from e
            key = (r.get("family"), r["id"], r.get("occurrence")) if kind == "stdlib" else r["id"]
            require(key not in seen, f"duplicate {kind} receipt: {key}")
            seen.add(key)
            if kind == "script":
                command = (r["source"]["repo"], r["source"]["commit"], r["source"]["path"], json.dumps(r["normalized_invocation"], sort_keys=True))
                require(command not in commands, f"duplicate original command: {r['id']}")
                commands.add(command)
            else:
                verify_stdlib_evidence(r)
            records.append(r)
    return records


def summary():
    fs = families()
    stdlib = load_records("stdlib", fs)
    scripts = load_records("script", fs)
    require(len(scripts) <= COHORT["scripts"]["expected_commands"], "more script receipts than original commands")
    for name, row in fs.items():
        rs = [r for r in stdlib if r["family"] == name]
        require(len(rs) <= row["originals"], f"{name}: receipts exceed raw denominator")
        row["receipts"] = len(rs)
        row["paired"] = sum(r["paired"] for r in rs)
        row["runs"] = {t: Counter(r["runs"][t]["outcome"] for r in rs) for t in TIERS}
        for t in TIERS: row["runs"][t]["not_run"] += row["originals"] - len(rs)
    return fs, scripts


def outcome_text(counts, n):
    if counts["not_run"] == n: return "not run"
    labels = {"pass":"pass", "fail":"fail", "error":"error", "skip":"skip", "timeout":"timeout", "not_run":"not run", "omitted":"omitted"}
    return ", ".join(f"{counts[k]:,} {labels[k]}" for k in labels if counts[k])


def render(view):
    fs, scripts = summary()
    total = sum(r["originals"] for r in fs.values())
    receipts = sum(r["receipts"] for r in fs.values())
    paired = sum(r["paired"] for r in fs.values())
    script_paired = sum(r["paired"] for r in scripts)
    expected = COHORT["scripts"]["expected_commands"]
    lines = [START, "### Pinned compatibility scorecard", "", f"Revision `{COHORT['imre_revision']}` · CPython **{COHORT['cpython_version']}** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.", "", f"Stdlib: **{paired:,} verified paired passes**, **{receipts:,} original-test receipts** of **{total:,} raw upstream originals**; **{total-receipts:,} originals have no receipt**. Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{script_paired} verified paired passes**; **{expected-len(scripts)} not run**.", "", "| Family | Raw originals | Stock | O0 | O3 | Paired at pin | Historical mixed-revision credits |", "| --- | ---: | --- | --- | --- | --- | ---: |"]
    selected = sorted(fs) if view == "stdlib" else sorted(n for n,r in fs.items() if r["receipts"] or n == "argparse")
    for name in selected:
        r = fs[name]; n = r["originals"]
        pair = "unknown (not run)" if not r["receipts"] else f"{r['paired']:,} verified" + (f"; {n-r['receipts']:,} unknown" if r["receipts"] < n else "")
        hist = "—" if r["historical"] is None else f"{r['historical']:,}"
        lines.append(f"| `{name}` | {n:,} | " + " | ".join(outcome_text(r["runs"][t], n) for t in TIERS) + f" | {pair} | {hist} |")
    if view == "readme": lines += ["", "The [stdlib report](docs/stdlib.md) lists every family, including those not run."]
    prefix = "../" if view == "stdlib" else ""
    lines += ["", f"A paired pass requires the same original to pass stock, O0 and O3; script passes also require exact exit, stdout, stderr and effect comparisons. Skips, omissions, unsupported diagnostics, exclusions, timeouts and non-stock successes never count. Historical credits come from the [frozen mixed-revision ledger]({prefix}{COHORT['historical_families']}) and are separate from the pinned result. [Receipts and replay instructions]({prefix}benchmarks/compatibility/README.md).", END]
    return "\n".join(lines)


def replace_block(text, block):
    require(text.count(START) == text.count(END) == 1, "expected exactly one scorecard block")
    before, rest = text.split(START); _, after = rest.split(END)
    return before + block + after


def main():
    p = argparse.ArgumentParser(description=__doc__); p.add_argument("--check", action="store_true"); p.add_argument("--validate", action="store_true"); args = p.parse_args()
    summary()
    if args.validate: return
    for path, view in ((ROOT / "README.md", "readme"), (ROOT / "docs/stdlib.md", "stdlib")):
        old = path.read_text(); new = replace_block(old, render(view))
        if args.check: require(old == new, f"stale scorecard: {path.relative_to(ROOT)}")
        else: path.write_text(new)


if __name__ == "__main__": main()
