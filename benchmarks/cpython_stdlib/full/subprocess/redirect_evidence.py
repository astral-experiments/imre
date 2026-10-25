"""Apply measured descriptor redirections to the full, paired Linux reports."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports

OFFSETS = dict(
    test_stdin_filedes=4,
    test_stdin_fileobj=4,
    test_stdout_filedes=6,
    test_stdout_fileobj=6,
    test_stderr_filedes=10,
    test_stderr_fileobj=10,
    test_stdout_stderr_file=11,
)
IDS = {
    f"test.test_subprocess.{cls}.{method}"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
    for method in OFFSETS
}


def observation(row, sample, snapshot):
    name = row["id"]
    method = name.rsplit(".", 1)[-1]
    form = "integer fd" if method.endswith("_filedes") else "native file object"
    assert row["reference"] == "pass" and row["occurrence"] == 1
    assert sample["representation"] == form
    assert sample["fixture_fd_offset_before"] == 0
    assert sample["fixture_fd_offset_after_wait"] == OFFSETS[method]
    assert sample["native_parents"] == 1 and sample["native_parent_selector_calls"] == 0
    assert sample["operations"][:2] == ["Popen", "Popen.wait"]
    assert sample["roles"] == dict(
        body="cpython", assertions="cpython", fixture="cpython", operation="imre"
    )
    return dict(
        outcome="pass",
        roles=sample["roles"],
        evidence=dict(
            opt_level=row["opt_level"],
            imre_revision=snapshot["imre_revision"],
            tested_tree=snapshot["tested_tree"],
            source="redirects-merged-linux.json",
            operation=f"native Popen used the original open file description via {form} and waited for the fixture child; the caller's offset advanced from 0 to {OFFSETS[method]}",
            fixture="original CPython test body, assertions, hooks and temporary file; CPython data child, no selector syscall in the compiled parent",
        ),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--previous", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    previous = json.loads(args.previous.read_text())
    snapshot = json.loads((HERE / "redirects-merged-linux.json").read_text())
    validate_pair(previous, ref)
    assert [r["opt_level"] for r in snapshot["reports"]] == [0, 3]
    result = []
    for before in previous:
        level = before["opt_level"]
        [raw] = [r for r in snapshot["reports"] if r["opt_level"] == level]
        assert raw["cpython_commit"] == ref["cpython_commit"]
        assert raw["rows"].keys() == IDS and raw["selector_control_count"] > 0
        obs, exceptions, discovery = {}, {}, []
        for row in before["records"]:
            name, occ = key = row["id"], row["occurrence"]
            discovery.append((name, row["fixture_variant"]))
            if "exception" in row:
                exceptions[name, occ, level] = row["exception"]
            if name in IDS:
                [sample] = raw["rows"][name]
                obs[key] = observation(row, sample, snapshot)
            else:
                obs[key] = {
                    k: row[k]
                    for k in ("outcome", "roles", "reason", "evidence")
                    if k in row
                }
        result.append(build_report(ref, level, discovery, obs, exceptions=exceptions))
    write_reports(args.result, result, ref)
    for report in result:
        print(report["opt_level"], report["denominator"], report["counts"])


if __name__ == "__main__":
    main()
