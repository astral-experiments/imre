"""Join independently measured subprocess roles to #560's per-occurrence contract."""

import argparse
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from report import REFERENCE_ONLY, compact
from selector_proof import CONTROL
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests

# A completed method earns native operation credit only after inspecting what
# its original assertions (or, in CONTROL, its retry loop) actually observe.
OPERATIONS = {
    "test_call_kwargs": "call with a child environment; wait for its exit status",
    "test_call_seq": "call with a sequence; wait for its exit status",
    "test_check_call_zero": "check_call returns the successful child's status",
    "test_stdin_pipe": "write and close child stdin; wait for the child's status",
    "test_stdout_pipe": "read the child's stdout",
    "test_stderr_pipe": "read the child's stderr",
    "test_stdout_stderr_pipe": "merge stderr into stdout and read the pipe",
    "test_communicate_stdin": "communicate with child stdin; observe the status",
    "test_communicate_stdout": "communicate and compare both result slots with stdout piped",
    "test_communicate_stderr": "communicate and compare both result slots with stderr piped",
    "test_communicate_returns": "communicate with inherited streams and compare both empty slots",
    "test_wait": "wait twice on one child and observe its status",
    "test_pipe": "read both child pipes and check that context exit closed them",
    "test_returncode": "read the status reaped by Popen context exit",
    "test_communicate_repeated_call_after_stdout_close": "retry timed communication on one child after its stdout reaches EOF",
}

# Per-occurrence source and runtime condition from the exact vendored file.
SKIPS = {
    "Windows specific tests": (3672, "Win32ProcessTestCase requires mswindows"),
    "Windows-specific tests": (4039, "CommandsWithSpaces requires mswindows"),
    "Windows only issue": (None, "the method requires sys.platform == 'win32'"),
    "need an installed Python. See #7774": (
        527,
        "sysconfig.is_python_build() is true for the pinned source build",
    ),
    "need msvcrt.CrtSetReportMode": (
        1544,
        "importing msvcrt or its CrtSetReportMode failed",
    ),
    "need subprocess._winapi": (1782, "subprocess has no _winapi on Linux"),
    "Maybe test trigger a leak on Ubuntu": (
        1946,
        "the upstream decorator requires mswindows",
    ),
    "setreuid() available on platform": (
        2263,
        "os.setreuid exists; the test covers its absence",
    ),
    "setregid() available on platform": (
        2310,
        "os.setregid exists; the test covers its absence",
    ),
    "setgroup() EPERM; this test may require root.": (
        2338,
        "the child setgroups call raised PermissionError under the reference account",
    ),
}


def read(path):
    return json.loads(path.read_text())


def diagnostic(reason):
    return re.sub(r"/tmp/tmp[^ /;:'\"]+", "<case>", reason)


def report(args, level, discovered, reference):
    index = (0, 3).index(level)
    sources = {
        k: read(getattr(args, k)[index]) for k in ("audit", "bodies", "trace", "vfork")
    }
    if any(int(data["opt_level"]) != level for data in sources.values()):
        raise ValueError("wrong optimization level")
    audit, bodies = sources["audit"]["rows"], sources["bodies"]["rows"]
    ids = {name for name, _ in discovered}
    if set(audit) != ids or set(bodies) != ids or len(ids) != len(discovered):
        raise ValueError("subprocess audit and complete pinned discovery differ")
    vfork = "test.test_subprocess.POSIXProcessTestCase.test_vfork_used_when_expected"
    measured = sources["vfork"]["rows"][vfork]
    if measured["status"] == "not-selected":
        raise ValueError("missing vfork fixture execution")
    bodies[vfork] = measured
    traced = sources["trace"]["ids"]
    if (
        sources["trace"]["mode"] != "trace"
        or not traced[CONTROL][0]["native_parent_syscalls"]
    ):
        raise ValueError("missing native selector positive control")
    log = args.run_log[index].read_text()
    runs = set(
        re.findall(
            r"^\w+ \((test\.test_subprocess\.RunFuncTestCase\.\w+)\) \.\.\. ok$",
            log,
            re.M,
        )
    )
    if len(runs) != 8 or "\nOK\n" not in log or not runs <= ids:
        raise ValueError("incomplete native run adapter")
    observations, exceptions = {}, {}
    for name, variant in discovered:
        key = (name, 1)  # the upstream loader discovered no repeated IDs in this suite
        method = name.rsplit(".", 1)[1]
        body, actor = bodies[name], compact(audit[name])
        status = actor["status"]
        ev = {"imre_revision": args.imre_revision}
        if "reason" in body:
            ev["native_body_diagnostic"] = diagnostic(body["reason"])
        if "operations" in actor:
            ev["native_actor_calls"] = actor["operations"]
        if status == "reference-skip":
            reason = actor["reason"]
            line, condition = SKIPS[reason]
            if line is None:
                line = {
                    "test_win32_duplicate_envs": 805,
                    "test_win32_invalid_env": 909,
                }.get(method)
                if line is None:
                    raise ValueError(f"unreviewed Windows-only method: {name}")
            exceptions[(*key, level)] = dict(
                kind="upstream-skip",
                reason=reason,
                condition=condition,
                source=f"Lib/test/test_subprocess.py:{line}",
                platform=reference["platform"],
                configuration="CPython 3.14.5 Linux source build, ordinary reference account",
                fixture_variant=variant,
            )
            continue
        if body["status"] == "body-completed-needs-operation-review":
            if "ProcessTestCaseNoPoll." in name or name == CONTROL:
                [trace] = traced[name]
                if trace["method_sha256"] != body["method_sha256"] or (
                    name != CONTROL and trace["native_parent_syscalls"]
                ):
                    raise ValueError(
                        f"selector trace does not establish fixture semantics: {name}"
                    )
                ev["native_parent_selector_syscalls"] = trace["native_parent_syscalls"]
            ev.update(
                operation=OPERATIONS[method],
                fixture=body["fixture_execution"],
                method_sha256=body["method_sha256"],
                opt_level=level,
            )
            loc = roles(
                body="imre",
                assertions="none" if name == CONTROL else "imre",
                operation="imre",
                fixture="mixed",
            )
            obs = dict(outcome="pass", roles=loc)
        elif name in runs:
            ev.update(
                operation="subprocess.run launched and waited for the child in Imre",
                fixture="native_subprocess.py reconstructed host case with the original method and host assertions",
                opt_level=level,
            )
            obs = dict(
                outcome="pass",
                roles=roles(
                    body="cpython",
                    assertions="cpython",
                    operation="imre",
                    fixture="cpython",
                ),
            )
        elif status in {"host-driven-native", "mixed"}:
            ev.update(
                operation=", ".join(actor["operations"]),
                fixture="original CPython module, class and instance hooks",
                opt_level=level,
            )
            if status == "mixed":
                ev["host_operation"] = actor["reason"]
            obs = dict(
                outcome="pass",
                roles=roles(
                    body="cpython",
                    assertions="cpython",
                    operation="mixed" if status == "mixed" else "imre",
                    fixture="cpython",
                ),
            )
        elif status == "reference-only":
            # The isolated host adapter ran, but only the reference product operation was observed.
            ev.update(
                operation=REFERENCE_ONLY[method],
                fixture="original CPython module, class and instance hooks",
            )
            obs = dict(
                outcome="pass",
                roles=roles(
                    body="cpython",
                    assertions="cpython",
                    operation="cpython",
                    fixture="cpython",
                ),
            )
        elif status in {"failed", "blocked"}:
            obs = dict(
                outcome="failure"
                if status == "failed" or body["status"] == "failed"
                else "blocked",
                roles=roles(
                    body="unknown",
                    assertions="unknown",
                    operation="unknown",
                    fixture="cpython",
                ),
                reason=diagnostic(actor["reason"]),
            )
        else:
            raise ValueError(f"unreviewed outcome {status}: {name}")
        observations[key] = {**obs, "evidence": ev}
    return build_report(
        reference, level, discovered, observations, exceptions=exceptions
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for group in ("audit", "bodies", "trace", "vfork", "run-log"):
        parser.add_argument(
            "--" + group, nargs=2, metavar=("O0", "O3"), type=Path, required=True
        )
    parser.add_argument("--imre-revision", required=True)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.imre_revision):
        parser.error("use the full Imre revision")
    manifest = read(HERE / "UPSTREAM.json")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = read(HERE / "reference-linux.json")
    discovered = [
        (case.id(), type(case).__qualname__)
        for case in cases(load_suite(manifest["module"]))
    ]
    reports = [report(args, level, discovered, reference) for level in (0, 3)]
    write_reports(args.result, reports, reference)
    for item in reports:
        print(f"-O{item['opt_level']}: {item['counts']}")


if __name__ == "__main__":
    main()
