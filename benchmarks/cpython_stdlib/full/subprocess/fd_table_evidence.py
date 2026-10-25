"""Apply the three measured descriptor-table occurrences without changing the reference."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports

PREFIX = "test.test_subprocess.POSIXProcessTestCase."
# Original fixture shape: close_fds, keep collection and count, borrowed stdio.
SHAPES = {
    PREFIX + "test_close_fds": [
        (False, None, 0, 0),
        (True, None, 0, 0),
        (True, "set", 8, 0),
    ],
    PREFIX + "test_pass_fds_inheritable": [(True, "tuple", 2, 0)],
    PREFIX + "test_pass_fds_redirected": [(True, "list", 3, 3)],
}
DETAILS = {
    "test_close_fds": "three native launches inherit all 11 original fds, close them all, then keep the original set of eight",
    "test_pass_fds_inheritable": "the native child inherits both original pipe fds; the original non-inheritable fd remains non-inheritable in the parent",
    "test_pass_fds_redirected": "three borrowed stdio fds survive exec both at 0/1/2 and at the original explicitly kept numbers",
}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "fd-table", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    a = p.parse_args()
    previous = json.loads(a.previous.read_text())
    data = json.loads(a.fd_table.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    result = []
    for prev in previous:
        level = prev["opt_level"]
        [raw] = [r for r in data["reports"] if r["opt_level"] == level]
        if (
            raw["cpython_commit"] != ref["cpython_commit"]
            or raw["rows"].keys() != SHAPES.keys()
        ):
            raise ValueError("wrong upstream snapshot or occurrences")
        discovered, obs, exceptions = [], {}, {}
        for row in prev["records"]:
            name, occ = key = row["id"], row["occurrence"]
            discovered.append((name, row["fixture_variant"]))
            if "exception" in row:
                exceptions[name, occ, level] = row["exception"]
            obs[key] = {
                k: row[k]
                for k in ("outcome", "roles", "reason", "evidence")
                if k in row
            }
            if name not in SHAPES:
                continue
            [case] = raw["rows"][name]
            ops = case["launches"]
            if not (
                occ == 1
                and row["reference"] == "pass"
                and case["body_attempt"]["status"] == "blocked"
                and case["roles"]
                == dict(
                    body="cpython",
                    assertions="cpython",
                    fixture="cpython",
                    operation="imre",
                )
            ):
                raise ValueError((name, case))
            shape = [
                (
                    op["close_fds"],
                    op["pass_fds_collection"],
                    op["kept_descriptors"],
                    op["borrowed_stdio"],
                )
                for op in ops
            ]
            if shape != SHAPES[name] or any(
                not op["parent_table_unchanged"]
                or op["native_reaps"] != 1
                or op["child_bulk_cloexec"] != op["close_fds"]
                or op["data_child"] != "original CPython fd_status.py"
                for op in ops
            ):
                raise ValueError((name, ops))
            method = name.rsplit(".", 1)[1]
            obs[key] = dict(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    opt_level=level,
                    imre_revision=data["imre_revision"],
                    tested_tree=data["tested_tree"],
                    source=a.fd_table.name,
                    operation=DETAILS[method]
                    + "; the native parent reaps each child and keeps every original descriptor and flag",
                    fixture="original CPython test body, assertions and OS fixtures; the original fd_status.py child reports the actual inherited numbers",
                    native_body_diagnostic=case["body_attempt"]["reason"],
                ),
            )
        result.append(build_report(ref, level, discovered, obs, exceptions=exceptions))
    write_reports(a.result, result, ref)
    for r in result:
        print(r["opt_level"], r["denominator"], r["counts"])


if __name__ == "__main__":
    main()
