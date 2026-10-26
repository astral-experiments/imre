"""Apply verified native operations to the existing per-occurrence reports."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from readiness_evidence import ID as READINESS_ID, verify_case
from suite_evidence import build_report, validate_pair


def selected(report, attempts, revision, tree):
    """Require original method identity, actual operations, and the observed fixture roles."""
    rows = {}
    host = "original CPython module, class and instance hooks"
    base = dict(
        opt_level=report["opt_level"],
        imre_revision=revision,
        tested_tree=tree,
        source="selected-operations-linux.json",
    )

    def save(name, roles, operation, fixture, **extra):
        if name in rows:
            raise ValueError(f"duplicate operation: {name}")
        rows[name] = dict(
            outcome="pass",
            roles=roles,
            evidence={**base, "operation": operation, "fixture": fixture, **extra},
        )

    def body(name, observed):
        attempt = attempts["rows"][name]
        assert attempt["status"] == "body-completed-needs-operation-review"
        assert attempt["method_sha256"] == observed["method_sha256"]
        assert observed["body"] == "Imre" and observed["assertions"] == "Imre"
        return observed["method_sha256"]

    checked = report["checked"]["rows"]
    assert len(checked) == 9
    for name, [case] in checked.items():
        method = name.rsplit(".", 1)[1]
        seen = case["observed"]
        digest = body(name, case["body"])
        if method == "test_invalid_cmd":
            assert (seen["nul_rejections"], seen["launched"]) == (2, 0)
            assert seen["runtime_calls"].count("imre_popen_config") == 2
            operation = (
                "the native Popen rejected both original NUL arguments before forking"
            )
        else:
            assert seen["native_reaps"] == seen["launched"] == 1
            expected = {
                "test_check_call_nonzero": 47,
                "test_check_output_nonzero": 5,
                "test_check_output_timeout": -9,
                "test_check_zero": 0,
            }[method]
            assert expected == seen["returncode"] and seen["data_child"]
            selectors = seen["native_selector_syscalls"]
            if expected < 0:
                assert selectors and set(selectors) <= (
                    {"select", "pselect6"} if "NoPoll." in name else {"poll", "ppoll"}
                )
            operation = (
                f"{seen['runtime_calls'][0]} launched {seen['data_child']}; "
                f"native parent reaped status {expected}"
            )
            if expected < 0:
                operation += f" after its {', '.join(selectors)} deadline and SIGKILL"
        roles = dict(case["roles"], fixture="mixed")
        save(
            name,
            roles,
            operation,
            host + "; native case instance and unchanged assertions",
            method_sha256=digest,
        )

    captured = report["checked_output"]["native_body_host_fixture"]
    assert len(captured) == 12
    for name, [case] in captured.items():
        proof = case["observation"]
        assert proof["roles"] == dict(
            body="imre", assertions="imre", operation="imre", fixture="mixed"
        )
        assert case["body"] == case["assertions"] == "Imre"
        assert case["method_sha256"] == proof["evidence"]["method_sha256"]
        [parent] = case["selector_parents"]
        signals = parent["native_parent_syscalls"]
        if name.endswith("test_check_output_stderr"):
            assert not signals  # The merged stream needs no multiplexing.
        else:
            assert signals and set(signals) <= (
                {"select", "pselect6"} if "NoPoll." in name else {"poll", "ppoll"}
            )
        save(
            name,
            proof["roles"],
            proof["evidence"]["operation"],
            host
            + "; native typed membership assertions and controlled inherited stdin",
            method_sha256=case["method_sha256"],
            native_parent_selector_syscalls=signals,
        )

    [(name, [case])] = report["low_fds"]["rows"].items()
    assert name.endswith(".test_small_errpipe_write_fd")
    digest = body(name, case)
    assert (
        case["native_closed_fds"]
        == case["launch_error_socket"]["fds"]
        == case["restored_with_dup2"]
        == [0, 1]
    )
    assert case["data_child"] == dict(
        engine="cpython", exec_observed=True, wait_status=0
    )
    save(
        name,
        dict(body="imre", assertions="none", operation="imre", fixture="mixed"),
        "native parent saved and closed fds 0/1, used both for its CLOEXEC launch socket, "
        "reaped the original Python child and restored both descriptors with dup2",
        host + "; the native original method has no explicit assertion",
        method_sha256=digest,
    )

    pipes = report["pipe_fixtures"]["rows"]
    assert len(pipes) == 3
    for name, [case] in pipes.items():
        op = case["observed"]
        assert case["body_attempt"]["status"] == "blocked"
        interrupted = name.endswith(".test_communicate_eintr")
        assert case["roles"] == dict(
            body="cpython",
            assertions="cpython",
            operation="imre" if interrupted else "mixed",
            fixture="mixed",
        )
        if interrupted:
            assert (
                op["native_deliveries"]
                == op["original_handler_calls"]
                == len(op["launches"])
                == 2
            )
            assert [x["stream"] for x in op["launches"]] == ["stdout", "stderr"]
            for launch in op["launches"]:
                assert (
                    launch["signal_return"] == "EINTR"
                    and launch["successful_reaps"] == 1
                )
                assert (
                    launch["restarted_result"] == "EOF"
                    and not launch["native_selector_syscalls"]
                )
            operation = "native communicate retried both interrupted reads on the same fd, reached EOF and reaped each original child"
            fixture = (
                host
                + "; both native SIGUSR1 deliveries invoked the original host handler before communicate returned"
            )
        else:
            assert name.endswith(".test_select_unbuffered")
            assert (
                op["read_bytes"],
                op["remaining_kernel_bytes"],
                op["native_reaps"],
            ) == (4, 1, 1)
            assert (
                op["native_stream_closed"]
                and op["same_open_file_description"] == "SCM_RIGHTS"
                and op["host_select_returned_original_stream"]
            )
            operation = "native unbuffered read consumed four bytes; host select returned the original stream with one byte remaining on the same open file description"
            fixture = (
                host
                + "; host select/cleanup and assertions, native read/close/wait and SCM_RIGHTS descriptor fixture"
            )
        save(
            name,
            case["roles"],
            operation,
            fixture,
            native_body_diagnostic=case["body_attempt"]["reason"],
        )
    return rows


def load(name):
    return json.loads(name.read_text())


def integrated_readiness(data, level, reference):
    """Keep the upstream host fixture and validate both observations of its pipe."""
    [raw] = [item for item in data["reports"] if item["opt_level"] == level]
    primary, transfer = raw["procfs"], raw["descriptor_transfer"]
    case = verify_case(primary)
    [other] = transfer["rows"][READINESS_ID]
    observed = other["observed"]
    stream, integer = (raw["operands"][key] for key in ("stream", "integer"))
    if not (
        data["cpython_commit"]
        == primary["cpython_commit"]
        == transfer["cpython_commit"]
        == reference["cpython_commit"]
        and primary["opt_level"] == transfer["opt_level"] == level
        and primary["toolchain"] == transfer["toolchain"]
        and other["body_attempt"] == case["body_attempt"]
        and other["roles"] == {**case["roles"], "fixture": "mixed"}
        and observed["same_open_file_description"] == "SCM_RIGHTS"
        and observed["read_bytes"] == 4
        and observed["remaining_kernel_bytes"] == observed["native_reaps"] == 1
        and observed["host_select_returned_original_stream"]
        and observed["native_stream_closed"]
        and stream["native"] == "compile-error"
        and "expected int, got typing.BinaryIO" in stream["diagnostic"]
        and integer["native"] == "pass"
        and integer["native_readiness_syscalls"] == integer["native_reaps"] == 1
        and stream["cpython"]
        == integer["cpython"]
        == integer["stdout"]
        == ["b'appl'", "True", "0"]
    ):
        raise ValueError(
            "readiness observations disagree with the original stream, fixtures or operand controls"
        )
    return dict(
        outcome="pass",
        roles=case["roles"],
        evidence=dict(
            opt_level=level,
            imre_revision=data["imre_revision"],
            tested_tree=data["tested_tree"],
            source="pipe-readiness-integrated-linux.json",
            operation="Imre launches the original CPython data child, reads four bytes, closes its raw pipe and reaps once. CPython select returns that stream with one byte left. Native select accepts its integer fd but rejects the stream operand used by the original.",
            fixture="CPython runs the original test, assertions and addCleanup. The independent SCM_RIGHTS fixture observes the same open file description. Held-open controls distinguish a queued byte from EOF.",
            native_body_diagnostic=case["body_attempt"]["reason"],
        ),
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--previous", type=Path, required=True)
    p.add_argument("--result", type=Path, required=True)
    p.add_argument(
        "--raw-directory",
        type=Path,
        help="read fresh driver results instead of the committed reports",
    )
    p.add_argument("--imre-revision")
    p.add_argument("--tested-tree")
    args = p.parse_args()
    if bool(args.raw_directory) != bool(args.imre_revision and args.tested_tree):
        p.error("supply the measured revision and tree with --raw-directory")
    ref = load(HERE / "reference-linux.json")
    readiness = load(HERE / "pipe-readiness-integrated-linux.json")
    previous = load(args.previous)
    validate_pair(previous, ref)
    if args.raw_directory:
        header = dict(
            cpython_commit=ref["cpython_commit"],
            imre_revision=args.imre_revision,
            tested_tree=args.tested_tree,
        )

        def raw(name, level):
            return load(args.raw_directory / f"{name}-{level}.json")

        full = dict(**header, reports=[raw("attempts", level) for level in (0, 3)])
        drivers = dict(
            checked="checked_error_originals",
            checked_output="native_checked_output",
            low_fds="low_errpipe",
            pipe_fixtures="pipe_fixture_originals",
        )
        sample = dict(
            **header,
            reports=[
                dict(
                    opt_level=level,
                    **{key: raw(driver, level) for key, driver in drivers.items()},
                )
                for level in (0, 3)
            ],
        )
        for name, result in (
            ("method-attempts-linux.json", full),
            ("selected-operations-linux.json", sample),
        ):
            (args.raw_directory / name).write_text(json.dumps(result, indent=2) + "\n")
    else:
        full = load(HERE / "method-attempts-linux.json")
        sample = load(HERE / "selected-operations-linux.json")
    for artifact in (full, sample):
        assert artifact["cpython_commit"] == ref["cpython_commit"]
        assert [x["opt_level"] for x in artifact["reports"]] == [0, 3]
    assert (full["imre_revision"], full["tested_tree"]) == (
        sample["imre_revision"],
        sample["tested_tree"],
    )
    out = []
    for old in previous:
        level = old["opt_level"]
        [attempts] = [r for r in full["reports"] if r["opt_level"] == level]
        [obs] = [r for r in sample["reports"] if r["opt_level"] == level]
        assert (
            set(attempts["rows"]) == set(ref["reference"])
            and attempts["denominator"] == ref["denominator"]
        )
        assert all(
            obs[k]["opt_level"] == level
            for k in ("checked", "checked_output", "low_fds", "pipe_fixtures")
        )
        new = selected(obs, attempts, sample["imre_revision"], sample["tested_tree"])
        assert len(new) == 25
        new[READINESS_ID] = integrated_readiness(readiness, level, ref)
        seen, exceptions, discovered = {}, {}, []
        for row in old["records"]:
            key = row["id"], row["occurrence"]
            discovered.append((key[0], row["fixture_variant"]))
            if "exception" in row:
                exceptions[key[0], key[1], level] = row["exception"]
            if key[0] in new:
                assert key[1] == 1 and row["reference"] == "pass"
                seen[key] = new.pop(key[0])
            else:
                seen[key] = {
                    k: row[k]
                    for k in ("outcome", "roles", "reason", "evidence")
                    if k in row
                }
        assert not new
        out.append(build_report(ref, level, discovered, seen, exceptions=exceptions))
    validate_pair(out, ref)
    args.result.write_text(json.dumps(out, indent=2) + "\n")
    for report in out:
        print(report["opt_level"], report["denominator"], report["counts"])


if __name__ == "__main__":
    main()
