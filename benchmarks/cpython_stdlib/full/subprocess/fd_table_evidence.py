"""Apply measured descriptor-table occurrences without changing the reference."""

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from suite_evidence import build_report, validate_pair, write_reports

HERE = Path(__file__).resolve().parent
PREFIX = "test.test_subprocess.POSIXProcessTestCase."
PASS_FDS = PREFIX + "test_pass_fds"
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
    "test_pass_fds": "the native parent passes exactly one of the fixture's ten inheritable fds per fd-status child; each following native call emits RuntimeWarning before launching the fixture's zero-return command",
}


def check_warning_pairs(ops):
    """Each original descriptor reaches both native APIs and exactly one warning."""
    if len(ops) != 20:
        raise ValueError("the original method must finish ten Popen/call pairs")
    descriptors = set()
    for pipe, call in zip(ops[::2], ops[1::2], strict=True):
        if not (
            pipe["data_child"] == "original CPython fd_status.py"
            and pipe["argument_type"] == "list"
            and pipe["native_warning"] is None
            and call["data_child"] == "original ZERO_RETURN_CMD"
            and call["argument_type"] == "tuple"
            and Path(call["data_executable"]).name == "true"
            and call["native_warning"]
            == dict(
                file="subprocess.py",
                line=862,
                category="RuntimeWarning",
                message="pass_fds overriding close_fds.",
                before_child_creation=True,
            )
            and pipe["kept_numbers"] == call["kept_numbers"]
            and all(
                op["original_open_fds"] == 10 and op["original_cloexec_fds"] == 0
                for op in (pipe, call)
            )
        ):
            raise ValueError(
                ("incomplete descriptor or warning observation", pipe, call)
            )
        [fd] = pipe["kept_numbers"]
        descriptors.add(fd)
    if len(descriptors) != 10:
        raise ValueError("the ten pairs must exercise ten distinct descriptors")


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
        if data["schema"] == "imre.subprocess.fd-table/v1":
            shapes = SHAPES
        elif data["schema"] in (
            "imre.subprocess.pass-fds-warning/v1",
            "imre.subprocess.fd-table-with-shared-warning/v1",
        ):
            shapes = {PASS_FDS: [(True, "tuple", 1, 0), (False, "tuple", 1, 0)] * 10}
            if data["schema"] == "imre.subprocess.fd-table-with-shared-warning/v1":
                shapes = SHAPES | shapes
            control = raw["withheld_warning"]
            if control[
                "original_failure"
            ] != "AssertionError: RuntimeWarning not triggered" or control[
                "native_warning"
            ] != dict(
                file="subprocess.py",
                line=862,
                category="RuntimeWarning",
                message="pass_fds overriding close_fds.",
            ):
                raise ValueError(
                    "withholding the native warning must fail the unchanged assertion"
                )
        else:
            raise ValueError("unknown descriptor report")
        if (
            raw["cpython_commit"] != ref["cpython_commit"]
            or raw["rows"].keys() != shapes.keys()
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
            if name not in shapes:
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
            if shape != shapes[name] or any(
                not op["parent_table_unchanged"]
                or op["native_reaps"] != 1
                or op["child_bulk_cloexec"]
                != (op["close_fds"] or bool(op["kept_descriptors"]))
                or (
                    name != PASS_FDS
                    and op["data_child"] != "original CPython fd_status.py"
                )
                for op in ops
            ):
                raise ValueError((name, ops))
            if name == PASS_FDS:
                check_warning_pairs(ops)
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
                    fixture=(
                        "original CPython body, assertions, OS fixture and assertWarns; the host forwards the actual native warning; the data children run fd_status.py on pinned CPython and the module's true command"
                        if name == PASS_FDS
                        else "original CPython test body, assertions and OS fixtures; the original fd_status.py child reports the actual inherited numbers"
                    ),
                    native_body_diagnostic=case["body_attempt"]["reason"],
                ),
            )
        result.append(build_report(ref, level, discovered, obs, exceptions=exceptions))
    write_reports(a.result, result, ref)
    for r in result:
        print(r["opt_level"], r["denominator"], r["counts"])


if __name__ == "__main__":
    main()
