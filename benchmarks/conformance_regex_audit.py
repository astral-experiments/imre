"""Independently replay interleaved scanner/report measurements and their CPython oracles."""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import runpy
import statistics
import sys

ENGINES = {"upstream", "cpython", "before", "after", "codon", "rust"}
PANELS = {"unchanged-github", "changed-github", "changed-diff"}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> None:
    if sys.flags.optimize:
        raise RuntimeError("auditing requires assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("matrix", type=Path)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--retained", type=Path, help="portable inputs/ directory containing suite, fixtures and report panels")
    parser.add_argument("--strict-files", action="store_true", help="rehash absolute machine-local executable/tool/source paths")
    parser.add_argument("--allow-pilot", action="store_true", help="audit an explicitly exploratory complete matrix")
    args = parser.parse_args()
    raw = args.matrix.read_bytes()
    m = json.loads(raw)
    assert m["completed"] and ((not m["pilot"] and m["config"]["source_locked"]) or args.allow_pilot)
    kernel = m.get("kernel", "scanner")
    baseline = {"scanner": "dad5a967fa7fa34cc34cddbc02e4ab35d147e0da",
                "classification": "0d61459957cf483404c48150c543032d87cf91c2",
                "entries": "490ff91eb4ac3d9af47e790259a7441bf8a6fa00"}[kernel]
    operation = {"scanner": "scan", "classification": "classify", "entries": "entries"}[kernel]
    assert m["revision"] == m["config"]["revision"] and m["baseline_revision"] == m["config"]["baseline_revision"] == baseline
    if kernel != "scanner":
        assert m["config"]["mode"] in ("normal", "thinlto")
        assert all(set(commands) == {operation, "report", "compute", "live"} for commands in m["config"]["engines"].values())
        assert m["pilot"] or m["samples"] == 12
    orders = m["orders"]
    assert len(orders) == 6 and m["samples"] >= 6 and m["samples"] % 6 == 0
    assert all(len(row) == 6 and set(row) == ENGINES for row in orders)
    assert all({row[i] for row in orders} == ENGINES for i in range(6))
    assert Counter(p for row in orders for p in zip(row, row[1:])) == {
        (a, b): 1 for a in ENGINES for b in ENGINES if a != b
    }
    strict = 0
    if args.strict_files:
        for item in [m["config_file"], *m["config"]["guarded_inputs"], m["runner"]]:
            b = Path(item["path"]).read_bytes()
            assert (digest(b), len(b)) == (item["sha256"], item["bytes"])
            strict += 1
    def read(item, relative):
        path = args.retained / relative if args.retained else Path(item["path"])
        b = path.read_bytes()
        assert (len(b), digest(b)) == (item["bytes"], item["sha256"]), path
        return b
    inputs = m["inputs"]
    assert len(inputs["test_files"]) == 146 and inputs["revision"] == "835b70f3f795f8e061b1f7621178515b0a77a0ad"
    texts = {f["path"]: read(f, Path("suite") / Path(f["path"]).name).decode() for f in inputs["test_files"]}
    lines = [line for text in texts.values() for line in text.splitlines()]
    validation = runpy.run_path(str(args.source_root / "benchmarks/conformance_validate.py"))
    pattern = validation["upstream"]().CONFORMANCE_ERROR_PATTERN
    scores = {}
    for name, fixture in inputs["fixtures"].items():
        if kernel != "scanner":
            continue
        text = read(fixture["input"], name + ".txt").decode().split("\0")
        if name == "corpus":
            assert text == lines and len(text) == 14_986
        output, total, score = [], 0, 0
        for index, line in enumerate(text):
            found = pattern.search(line)
            if found is None:
                continue
            begin, end = found.span()
            tag, description = found["tag"] or "", found["description"] or "Missing"
            optional, multi = int(found["optional"] is not None), int(found["multi"] is not None)
            total += 1
            score += 1 + begin + 3*end + 5*len(description) + 7*len(tag) + 11*optional + 13*multi
            output.append(f"{index}:{begin}:{end}:{optional}:{multi}:{len(tag)}:{tag}{description}")
        output.append(f"{total} {score}")
        transcript = ("\n".join(output) + "\n").encode()
        assert (len(text), total, score) == (fixture["lines"], fixture["matches"], fixture["checksum"])
        assert read(fixture["capture_oracle"], name + ".captures") == transcript
        assert m["capture_validation"][name] == {engine: digest(transcript) for engine in ENGINES}
        scores[name] = (total, score)
    assert set(m["panels"]) == PANELS
    reports = {}
    for name, panel in m["panels"].items():
        captures = panel["captures"]
        for key, item in captures.items():
            read(item, Path(name) / (key + ".json"))
        meta = json.loads(read(panel["preparation"], Path(name) / "preparation.json"))
        assert all(captures[role]["sha256"] == meta["captures"][role]["stdout_sha256"] for role in ("old", "new"))
        def location(role):
            return args.retained/name/(role+".json") if args.retained else Path(captures[role]["path"])
        workload = validation["Workload"](location("manifest"), location("old"), location("new"), name.split("-")[1])
        if name == "changed-github" and kernel != "scanner":
            helper = runpy.run_path(str(args.source_root / "benchmarks/conformance_classification_upstream.py"))
            transcript, distribution = helper["snapshot"](validation, workload, texts, kernel=kernel)
            fixture = inputs["fixtures"]["corpus"]
            n, score = map(int, transcript.splitlines()[-1].split())
            assert (n, score, distribution) == (fixture["cases"], fixture["checksum"], fixture["distribution"])
            assert read(fixture["capture_oracle"], "corpus.captures") == transcript
            assert m["capture_validation"]["corpus"] == {engine: digest(transcript) for engine in ENGINES}
            scores["corpus"] = n, score
        original = validation["oracle"](workload, source_contents=texts).report
        assert original is not None and original.encode() == read(panel["expected_report"], name + ".md")
        assert len(original) == panel["expected_report"]["characters"]
        reports[name] = original
    expected_cases = {kernel, kernel + "-preload"} | {
        p+"/"+k for p in PANELS for k in ("live-checkout", "complete-report", "repeated-report", "report-preload")
    }
    assert set(m["cases"]) == expected_cases
    times, all_ratios, timed, warmups = {}, {}, 0, 0
    for name, case in m["cases"].items():
        rounds = case["rounds"]
        if kernel != "scanner":
            role = name.split("/")[-1]
            expected = {kernel: (operation, None), kernel + "-preload": (operation, 0),
                        "complete-report": ("report", 1), "report-preload": ("compute", 0),
                        "repeated-report": ("compute", None), "live-checkout": ("live", 1)}
            entry, fixed_rounds = expected[role]
            assert case["executable"] == entry and case["control"] == (rounds == 0)
            assert rounds >= 2 if fixed_rounds is None else rounds == fixed_rounds
        if case["executable"] == operation:
            n, score = scores["corpus"]
            count = n if kernel != "scanner" else n * rounds
            stdout = output = f"{count} {score*rounds}\n".encode()
        else:
            report = reports[case["panel"]]
            stdout = f"{len(report)*rounds}\n".encode() if case["executable"] == "compute" else b""
            output = report.encode() if rounds else b""
        assert [s["engine"] for s in case["warmup"]] == orders[0][::-1]
        assert len(case["runs"]) == m["samples"]
        assert [[s["engine"] for s in row] for row in case["runs"]] == [
            orders[i % 6] for i in range(m["samples"])
        ]
        for s in [*case["warmup"], *(s for row in case["runs"] for s in row)]:
            assert s["elapsed_ns"] > 0 and s["output_sha256"] == digest(output) and s["stdout_sha256"] == digest(stdout)
        medians = {engine: statistics.median(s["elapsed_ns"] for row in case["runs"] for s in row if s["engine"] == engine)
                   for engine in ENGINES}
        assert medians == case["medians_ns"]
        ratio = medians["after"] / medians["rust"]
        assert ratio == case["after_over_rust"] and medians["after"] / medians["before"] == case["after_over_before"]
        times[name] = medians
        if not case["control"]:
            all_ratios[name] = ratio
        timed += 6 * m["samples"]
        warmups += 6
    pairs = [(kernel, kernel + "-preload")] + [(p+"/repeated-report", p+"/report-preload") for p in PANELS]
    assert set(m["per_round_ns"]) == {case for case, _ in pairs}
    for case, control in pairs:
        net = {e: (times[case][e] - times[control][e]) / m["cases"][case]["rounds"] for e in ENGINES}
        assert net == m["per_round_ns"][case] and min(net.values()) > 0
        all_ratios[case + "/net"] = net["after"] / net["rust"]
    gated = [r for n, r in all_ratios.items() if kernel != "entries" or n.split("/")[0] in PANELS]
    assert m["time_target_met"] == all(0 < r <= 2 for r in gated)
    if kernel != "scanner" and not m["pilot"] and not m["time_target_met"]:
        raise SystemExit("completed conformance comparison exceeds 2x Rust")
    print(json.dumps({"matrix_sha256": digest(raw), "pilot": m["pilot"], "timed": timed, "warmups": warmups,
                      "strict_files_checked": strict, "matched_capture_transcripts": len(inputs["fixtures"])*6,
                      "real_source_files": len(texts), "reconstructed_reports": sorted(reports),
                      "time_target_met": m["time_target_met"], "ratios": all_ratios}, indent=2))


if __name__ == "__main__":
    main()
