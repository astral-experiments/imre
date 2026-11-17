"""Validate compatibility receipts and render both public scorecard views."""
from __future__ import annotations

import argparse
import base64
import binascii
import csv
import hashlib
import json
import re
from collections import Counter
from functools import lru_cache
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
RESULT = re.compile(r"^([^\s]+) \(([^)]+)\) \.\.\. (ok|FAIL|ERROR|skipped .+|expected failure|unexpected success)$")
START = "<!-- compatibility-scorecard:start -->"
END = "<!-- compatibility-scorecard:end -->"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def families():
    with (ROOT / COHORT["historical_families"]).open(newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    result = {r["family"]: {"originals": int(r["originals"]), "historical": int(r["paired"]), "cohort": "frozen"} for r in rows}
    require(len(result) == len(rows), "duplicate historical family")
    for r in COHORT["supplemental_families"]:
        require(r["family"] not in result, "duplicate supplemental family")
        result[r["family"]] = {"originals": r["originals"], "historical": None, "cohort": "supplemental"}
    return result


def raw_observation(obj, name, check, required=False):
    keys = [key for key in (name, name + "_base64", name + "_path") if key in obj]
    digest_key = "effects_sha256" if name == "effects_manifest" else name + "_sha256"
    digest = obj.get(digest_key)
    if not keys and digest is None and not required: return None
    check(len(keys) == 1 and isinstance(digest, str) and bool(HEX64.fullmatch(digest)), f"{name}: need one raw representation and SHA-256")
    key = keys[0]; value = obj[key]
    check(isinstance(value, str), f"{key}: expected string")
    if key == name: data = value.encode("utf-8")
    elif key.endswith("_base64"):
        try: data = base64.b64decode(value, validate=True)
        except (ValueError, binascii.Error): check(False, f"{key}: invalid base64")
    else:
        path = (ROOT / value).resolve()
        check(path.is_relative_to(ROOT.resolve()) and path.is_file(), f"{key}: missing repository-local raw file")
        data = path.read_bytes()
    check(hashlib.sha256(data).hexdigest() == digest, f"{name}: digest inconsistent with raw bytes")
    if name == "effects_manifest":
        try: manifest = json.loads(data)
        except (ValueError, UnicodeError): check(False, "effects_manifest: expected raw JSON object")
        check(isinstance(manifest, dict), "effects_manifest: expected raw JSON object")
    return data


def validate_observations(obj, check, required=False):
    if required: check(type(obj.get("exit_code")) is int, "missing exit code")
    for name in ("stdout", "stderr", "effects_manifest"):
        raw_observation(obj, name, check, required)


def valid_preimages(preimages, check):
    check(isinstance(preimages, dict) and all(isinstance(k, str) and k and not k.startswith("/") and ".." not in Path(k).parts and (v is None or isinstance(v, str) and HEX64.fullmatch(v)) for k, v in preimages.items()), "invalid fixture preimages")


def validate_frozen_anchor(anchor):
    label = anchor.get("id", "<missing id>")
    def check(c, msg): require(c, f"frozen {label}: {msg}")
    check(anchor.get("schema") == "imre.compatibility.frozen-script.v1" and isinstance(label, str) and bool(label), "invalid schema or id")
    source = anchor.get("source", {})
    check(all(isinstance(source.get(k), str) and source[k] for k in ("repo", "commit", "path", "sha256")) and bool(HEX40.fullmatch(source["commit"])) and bool(HEX64.fullmatch(source["sha256"])), "invalid source")
    evidence = anchor.get("evidence", {})
    check(isinstance(evidence, dict) and isinstance(evidence.get("locator"), str) and bool(evidence["locator"]), "missing frozen evidence locator")
    data = raw_observation(evidence, "raw", check, True)
    try: frozen = json.loads(data)
    except (ValueError, UnicodeError): check(False, "frozen evidence must be JSON")
    check(isinstance(frozen, dict) and isinstance(frozen.get("fixture_scope"), str) and bool(frozen["fixture_scope"]), "missing frozen fixture scope")
    check(frozen.get("id") == label and frozen.get("source") == source and frozen.get("normalized_invocation") == anchor.get("normalized_invocation") and isinstance(anchor.get("normalized_invocation"), dict), "frozen evidence does not bind the original command")
    valid_preimages(frozen.get("preimages"), check)
    reference = frozen.get("reference")
    check(isinstance(reference, dict), "missing frozen reference")
    validate_observations(reference, check, True)
    return frozen


def load_frozen_scripts():
    path = HERE / "frozen" / "scripts.jsonl"
    anchors = {}
    if path.exists():
        for line in path.read_text().splitlines():
            anchor = json.loads(line); validate_frozen_anchor(anchor)
            require(anchor["id"] not in anchors, f"duplicate frozen script anchor: {anchor['id']}")
            anchors[anchor["id"]] = anchor
    return anchors


def validate_record(r, kind, family_names, frozen_scripts=None):
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
        if kind == "script":
            validate_observations(run, lambda c, msg: check(c, f"{tier} {msg}"), run["outcome"] == "pass")
    omission = r.get("omission_reason")
    check("omission_reason" in r and (omission is None or isinstance(omission, str) and bool(omission)), "invalid omission reason")
    check(not any(x["outcome"] == "omitted" for x in runs.values()) or bool(omission), "omission requires reason")
    paired = not omission and all(runs[t]["outcome"] == "pass" for t in TIERS)
    if kind == "script":
        provenance = r.get("fixture_provenance", {})
        check(isinstance(provenance, dict) and provenance.get("status") in {"exact_frozen", "reconstructed", "unknown"}, "missing fixture provenance status")
        observed = provenance.get("preimages", {})
        check(isinstance(observed, dict) and set(observed) <= set(TIERS), "invalid observed preimage tiers")
        for preimages in observed.values(): valid_preimages(preimages, check)
        established = False
        if provenance["status"] == "exact_frozen":
            anchor = (frozen_scripts or {}).get(label)
            check(anchor is not None and anchor.get("source") == source and anchor.get("normalized_invocation") == invocation, "exact frozen fixture requires an independent matching anchor")
            frozen = validate_frozen_anchor(anchor)
            check(set(observed) == set(TIERS), "exact frozen fixture needs preimages for all tiers")
            established = all(observed[t] == frozen["preimages"] for t in TIERS)
            reference = frozen["reference"]
            established = established and all(runs["stock"].get(field) == reference.get(field) for field in ("exit_code", "stdout_sha256", "stderr_sha256", "effects_sha256"))
        paired = paired and established
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


def parse_result(line, module):
    match = RESULT.fullmatch(line)
    if not match: return None
    test_id, status = match.group(2), match.group(3)
    if test_id.startswith("__main__."):
        test_id = module + test_id[len("__main__"):]
    outcome = "pass" if status == "ok" else "skip" if status.startswith("skipped ") else "fail" if status in {"FAIL", "unexpected success", "expected failure"} else "error"
    return test_id, outcome


def source_module(source_path):
    path = Path(source_path)
    require(path.parts[:1] == ("Lib",) and path.suffix == ".py", f"invalid CPython test source path: {source_path}")
    parts = list(path.with_suffix("").parts[1:])
    if parts[-1] == "__init__": parts.pop()
    return ".".join(parts)


@lru_cache(maxsize=None)
def evidence_index(path, module, mtime_ns, size):
    occurrences = Counter(); results = {}
    for line, text in enumerate(path.read_text().splitlines(), 1):
        result = parse_result(text, module)
        if result:
            test_id, outcome = result
            occurrences[test_id] += 1
            results[line] = (test_id, occurrences[test_id], outcome)
    return results


@lru_cache(maxsize=None)
def source_digests(path, mtime_ns, size):
    data = path.read_bytes()
    return hashlib.sha256(data).hexdigest(), hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def verify_stdlib_evidence(r):
    source = r["source"]
    vendored = ROOT / "benchmarks/cpython_stdlib/full" / r["family"] / "upstream" / source["path"]
    if vendored.is_file():
        stat = vendored.stat()
        require(source_digests(vendored, stat.st_mtime_ns, stat.st_size) == (source["sha256"], source["git_blob"]), f"{r['id']}: source hash differs from vendored upstream")
    module = source_module(source["path"])
    for tier, run in r["runs"].items():
        if run["outcome"] not in {"pass", "fail", "error", "skip"}: continue
        evidence = run["evidence"]
        match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", evidence)
        require(bool(match), f"{r['id']}: {tier} needs a raw evidence line")
        path = (ROOT / match.group(1)).resolve()
        require(path.is_relative_to(ROOT.resolve()) and path.is_file(), f"{r['id']}: missing evidence {evidence}")
        stat = path.stat(); line = int(match.group(2))
        result = evidence_index(path, module, stat.st_mtime_ns, stat.st_size).get(line)
        require(result == (r["id"], r["occurrence"], run["outcome"]), f"{r['id']}: {tier} evidence does not establish exact ID, occurrence and outcome")


def load_records(kind, names, frozen_scripts=None):
    records = []
    seen = set()
    commands = set()
    for path in sorted((HERE / ("stdlib" if kind == "stdlib" else "scripts")).glob("*.jsonl")):
        if kind == "script": require(path.stem in COHORT["scripts"]["shards"], f"unrecognized script shard: {path}")
        for line_num, line in enumerate(path.read_text().splitlines(), 1):
            try: r = validate_record(json.loads(line), kind, names, frozen_scripts)
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


def summarize_family(row, records):
    require(len(records) <= row["originals"], "receipts exceed raw denominator")
    row["receipts"] = len(records)
    row["observed"] = sum(all(r["runs"][t]["outcome"] == "pass" for t in TIERS) for r in records)
    row["paired"] = sum(r["paired"] for r in records)
    row["runs"] = {t: Counter(r["runs"][t]["outcome"] for r in records) for t in TIERS}
    for t in TIERS: row["runs"][t]["not_run"] += row["originals"] - len(records)


def summary():
    fs = families()
    stdlib = load_records("stdlib", fs)
    scripts = load_records("script", fs, load_frozen_scripts())
    require(len(scripts) <= COHORT["scripts"]["expected_commands"], "more script receipts than original commands")
    grouped = {name: [] for name in fs}
    for r in stdlib: grouped[r["family"]].append(r)
    for name, row in fs.items(): summarize_family(row, grouped[name])
    return fs, scripts


def outcome_text(counts, n):
    if counts["not_run"] == n: return "not run"
    labels = {"pass":"pass", "fail":"fail", "error":"error", "skip":"skip", "timeout":"timeout", "not_run":"not run", "omitted":"omitted"}
    return ", ".join(f"{counts[k]:,} {labels[k]}" for k in labels if counts[k])


def render(view):
    fs, scripts = summary()
    frozen = {n: r for n, r in fs.items() if r["cohort"] == "frozen"}
    supplemental = {n: r for n, r in fs.items() if r["cohort"] == "supplemental"}
    def totals(rows):
        return tuple(sum(r[k] for r in rows.values()) for k in ("originals", "receipts", "observed", "paired"))
    raw, received, observed, paired = totals(frozen)
    sup_raw, sup_received, sup_observed, sup_paired = totals(supplemental)
    expected = COHORT["scripts"]["expected_commands"]
    lines = [START, "### Pinned compatibility scorecard", "", f"Revision `{COHORT['imre_revision']}` · CPython **{COHORT['cpython_version']}** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.", "", f"Frozen {len(frozen)}-family cohort: **{received:,} pinned receipts** of **{raw:,} raw upstream originals**, **{observed:,} observed stock/O0/O3 pass intersections**, **{paired:,} verified credited pairs**; **{raw-received:,} originals have no receipt**.", "", f"Supplemental pilots, outside the frozen cohort: **{sup_received:,} receipts** of **{sup_raw:,} originals**, **{sup_observed:,} observed intersections**, **{sup_paired:,} verified credited pairs**.", "", f"Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{sum(r['paired'] for r in scripts)} verified paired passes**; **{expected-len(scripts)} not run**."]
    def table(title, rows):
        lines.extend(["", f"#### {title}", "", "| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |", "| --- | ---: | --- | --- | --- | ---: | --- | ---: |"])
        for name, r in sorted(rows.items()):
            n = r["originals"]
            pair = "unknown (not run)" if not r["receipts"] else f"{r['paired']:,} verified" + (f"; {n-r['receipts']:,} unknown" if r["receipts"] < n else "")
            observed_cell = "unknown (not run)" if not r["receipts"] else f"{r['observed']:,}"
            hist = "—" if r["historical"] is None else f"{r['historical']:,}"
            lines.append(f"| `{name}` | {n:,} | " + " | ".join(outcome_text(r["runs"][t], n) for t in TIERS) + f" | {observed_cell} | {pair} | {hist} |")
    table("Frozen cohort", frozen if view == "stdlib" else {n: r for n, r in frozen.items() if r["receipts"] or n == "argparse"})
    if supplemental: table("Supplemental pilots", supplemental)
    if view == "readme": lines += ["", "The [stdlib report](docs/stdlib.md) lists every frozen family, including those not run."]
    prefix = "../" if view == "stdlib" else ""
    lines += ["", f"An observed intersection means the same original passed stock, O0 and O3; frozen credited pairs also require the launcher to pass its imported bad-type control and any required source-withdrawal check. Script passes also require independently anchored frozen preimages and reference observations, plus exact exit, stdout, stderr and effect comparisons. Skips, omissions, unsupported diagnostics, exclusions, timeouts and non-stock successes never earn credited pairs. Historical credits come from the [frozen mixed-revision ledger]({prefix}{COHORT['historical_families']}) and are separate from the pinned result. [Receipts and replay instructions]({prefix}benchmarks/compatibility/README.md).", END]
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
