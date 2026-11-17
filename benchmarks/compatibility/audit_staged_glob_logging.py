"""Audit the unactivated 8571ca2 glob/logging stage without granting credit."""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path

import collect_unittest_results
import scorecard

PIN = "8571ca2dbecabf8d7863c369dc776611fd57d8ad"
CPYTHON = "5607950ef232dad16d75c0cf53101d9649d89115"
SNAPSHOT = scorecard.HERE / "revisions" / PIN
AUDIT = SNAPSHOT / "evidence" / "glob-logging-audit"
STAGES = {
    "glob": "61ec05c7664af546ab94a573a3ba97457b34e4a08f1f881761fad06788806a9c",
    "logging": "4dded3145629d25c5e7c6a74e0f386c94408d3338dc851dbedd1c6df03c1a210",
}
TIERS = ("stock", "O0", "O3")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def exact_result(row, tier):
    run = row["runs"][tier]
    locator, number = run["evidence"].rsplit("#L", 1)
    path = scorecard.repository_file(locator, SNAPSHOT)
    stat = path.stat()
    observed = scorecard.evidence_index(
        path, scorecard.source_module(row["source"]["path"]), stat.st_mtime_ns, stat.st_size
    ).get(int(number))
    require(observed == (row["id"], row["occurrence"], run["outcome"]),
            f"{tier}: staged raw line does not establish exact ID, occurrence and outcome")


def incomplete_evidence(row, tier):
    run = row["runs"][tier]
    path = scorecard.repository_file(run["evidence"], SNAPSHOT)
    evidence_dir = SNAPSHOT / "evidence" / row["family"]
    if run["outcome"] == "timeout":
        runs = json.loads((evidence_dir / f"{tier}.runs.json").read_text())
        matches = [r for r in runs if (r.get("id"), r.get("occurrence")) == (row["id"], row["occurrence"])]
        require(len(matches) == 1 and matches[0]["timed_out"] is True and matches[0]["exit_code"] != 0 and
                f"=== {row['id']} occurrence {row['occurrence']} ===" in path.read_text(),
                f"{tier}: timeout lacks exact original run evidence")
    else:
        build = json.loads((evidence_dir / f"{tier}.build.json").read_text())
        require(run["outcome"] == "not_run" and build["exit_code"] != 0 and
                build["executable_exists"] is False and sha(path) == build["stderr_sha256"],
                f"{tier}: not-run build evidence differs")


def check_source(row, cpython_root):
    if cpython_root is None:
        return
    path = cpython_root / row["source"]["path"]
    stat = path.stat()
    require(scorecard.source_digests(path, stat.st_mtime_ns, stat.st_size) ==
            (row["source"]["sha256"], row["source"]["git_blob"]), "upstream source digests differ")


def check_logging_leaf(cpython_root):
    if cpython_root is None:
        return
    original = (cpython_root / "Lib/test/test_logging.py").read_text()
    leaf = (SNAPSHOT / "evidence/logging/leaf/launcher.py").read_text()
    original_tree, leaf_tree = ast.parse(original), ast.parse(leaf)
    get_class = lambda tree: next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "BufferingFormatterTest")
    a, b = get_class(original_tree), get_class(leaf_tree)
    require(original.splitlines()[a.lineno - 1] == leaf.splitlines()[b.lineno - 1], "leaf class header differs")
    for name in ("setUp", "test_default"):
        left = next(n for n in a.body if isinstance(n, ast.FunctionDef) and n.name == name)
        right = next(n for n in b.body if isinstance(n, ast.FunctionDef) and n.name == name)
        require(ast.get_source_segment(original, left) == ast.get_source_segment(leaf, right), f"leaf {name} differs")
    get_fixture = lambda tree: next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "setUpModule")
    require(ast.get_source_segment(original, get_fixture(original_tree)) ==
            ast.get_source_segment(leaf, get_fixture(leaf_tree)), "leaf module fixture differs")
    # This is a reduced launcher, not the intact original class or file.
    require(ast.dump(a) != ast.dump(b), "unexpected intact logging class")


def audit(cpython_root=None):
    cohort = json.loads((SNAPSHOT / "cohort.json").read_text())
    activation = json.loads((SNAPSHOT / "activation.json").read_text())
    require(cohort["imre_revision"] == activation["imre_revision"] == PIN and
            cohort["cpython_commit"] == CPYTHON and cohort["cpython_version"] == "3.14.5",
            "snapshot or CPython pin differs")
    families = scorecard.families()
    result = {"schema": "imre.compatibility.staged-audit.v1", "imre_revision": PIN,
              "cpython_commit": CPYTHON, "families": {}}
    selected = []
    for family, digest in STAGES.items():
        path = SNAPSHOT / "stdlib" / (family + ".jsonl")
        require(sha(path) == digest, f"{family}: original staged bytes changed")
        require(family not in activation["families"], f"{family}: stage is no longer unactivated")
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        seen, candidates, other = set(), [], []
        for row in rows:
            key = row["id"], row["occurrence"]
            require(key not in seen, "duplicate original ID/occurrence")
            seen.add(key)
            require(row["family"] == family and row["kind"] == "stdlib" and
                    row["imre_revision"] == PIN and row["source"]["commit"] == CPYTHON and
                    row["platform"]["os"] == "linux" and row["platform"]["arch"] == "x86_64",
                    "stage pin, source, family or platform differs")
            # Only identity and source are compared to the frozen inventory. No
            # historical native outcome, credit or compiler artifact is read.
            scorecard.canonical_original(row, "stdlib")
            check_source(row, cpython_root)
            outcomes = {t: row["runs"][t]["outcome"] for t in TIERS}
            for tier in TIERS:
                if outcomes[tier] in {"pass", "fail", "error", "skip"}:
                    exact_result(row, tier)
                else:
                    incomplete_evidence(row, tier)
            item = {"id": row["id"], "occurrence": row["occurrence"], "outcomes": outcomes,
                    "evidence": {t: row["runs"][t]["evidence"] for t in TIERS}}
            if all(outcomes[t] == "pass" for t in TIERS):
                candidates.append(item)
                if family == "glob":
                    selected.append(key)
            else:
                other.append(item)
        source = rows[0]["source"]
        result["families"][family] = {
            "frozen_originals": families[family]["originals"], "staged_receipts": len(rows),
            "missing_staged_ids": families[family]["originals"] - len(rows),
            "stage_sha256": digest, "source": source,
            "public_source_anchor": f"https://github.com/python/cpython/blob/{CPYTHON}/{source['path']}",
            "candidate_intersections": candidates, "other_staged_outcomes": other,
            "validated_receipts": 0, "verified_pairs": 0,
        }
    check_logging_leaf(cpython_root)
    capture = json.loads((AUDIT / "glob-stock.json").read_text())
    require(capture["imre_revision"] == PIN and capture["source_commit"] == CPYTHON and
            capture["source_sha256"] == result["families"]["glob"]["source"]["sha256"] and
            capture["cpython_version"].startswith("Python 3.14.5 ") and capture["exit_code"] == 0 and
            not capture["timed_out"] and capture["native_status"] == "not_run",
            "fresh stock capture metadata differs")
    for file, key in (("glob-stock-launcher.py", "launcher_sha256"), ("glob-stock.stdout", "stdout_sha256"),
                      ("glob-stock.stderr", "stderr_sha256")):
        require(sha(AUDIT / file) == capture[key], f"fresh stock {file} digest differs")
    parsed = collect_unittest_results.parse((AUDIT / "glob-stock.stderr").read_bytes(), False)
    require(parsed["inventory_complete"] and parsed["summary"] == {"event": "summary", "run": len(selected), "successful": True}
            and not parsed["fixtures"] and not parsed["subtest_skips"] and
            [(r["id"], r["occurrence"]) for r in parsed["originals"]] == selected and
            all(r["outcome"] == "pass" for r in parsed["originals"]), "fresh stock inventory/outcomes differ")
    build = json.loads((AUDIT / "pinned-build.json").read_text())
    require(build["source_revision"] == PIN and build["source_checkout_clean"] and build["exit_code"] == 0 and
            build["imre_executed"] is False and sha(AUDIT / "pinned-build.raw") == build["build_raw_sha256"] and
            "--offline" in build["build_argv"] and "--locked" in build["build_argv"], "pinned build evidence differs")
    result["fresh_stock_only_glob_passes"] = len(selected)
    result["strict_pairs_added"] = 0
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-root", type=Path, help="optional exact CPython source tree; verifies SHA-256, Git blobs and leaf source")
    parser.add_argument("--check", action="store_true", help="compare with the committed audit.json")
    args = parser.parse_args()
    rendered = json.dumps(audit(args.cpython_root), indent=2) + "\n"
    if args.check:
        require((AUDIT / "audit.json").read_text() == rendered, "audit.json is stale")
        print("staged audit verified; no credit added")
    else:
        print(rendered, end="")


if __name__ == "__main__":
    main()
