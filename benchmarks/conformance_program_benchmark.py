"""Compare the unchanged Ruff program with native stdout ports on the same checkouts."""

import argparse
import gzip
import json
import os
import platform
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from contextlib import nullcontext
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from conformance_grouping_inputs import REVISION, read, write
from conformance_regex_benchmark import file_id, orders
from conformance_validate import APP, digest, environment


def replay(fixtures, checkout, scale, work):
    """Materialize discoverable files and replay the real, rebased ty JSON on stdout."""
    panel = fixtures / f"{scale}x/changed"
    sources = read(panel / "manifest.json")
    original = read(fixtures / "provenance.json")
    assert sources["suite_revision"] == original["revision"] == REVISION
    assert original["panels"][f"{scale}x/changed"]["inputs"] == {
        role: digest((panel / f"{role}.json").read_bytes())
        for role in ("manifest", "old", "new")
    }
    suite = work / f"{scale}x"
    tests = suite / "tests"
    tests.mkdir(parents=True)
    for name in sources["test_files"]:
        src = Path(name)
        prefix, separator, suffix = src.name.partition("_")
        original_name = (
            suffix
            if separator and prefix in {f"copy{i:02d}" for i in range(1, scale)}
            else src.name
        )
        assert digest(src.read_bytes()) == original["sources"][original_name]
        shutil.copyfile(src, tests / src.name)
    (suite / "src").mkdir()
    groups = (checkout / "src/test_groups.toml").read_bytes()
    for replica in range(1, scale):
        groups += f'\n[copy{replica:02d}]\nname = "replica"\nhref = ""\n'.encode()
    (suite / "src/test_groups.toml").write_bytes(groups)
    aliases = {d["raw"]: Path(d["path"]).name for d in sources["diagnostic_paths"]}
    children, captures = [], {}
    for side in ("old", "new"):
        diagnostics = read(panel / f"{side}.json")
        for diagnostic in diagnostics:
            loc = diagnostic["location"]
            if diagnostic["severity"] == "major" and loc["path"] not in aliases:
                raise ValueError(f"unknown major source {loc['path']}")
            loc["path"] = "tests/" + aliases.get(loc["path"], Path(loc["path"]).name)
        payload = suite / f"{side}.json"
        write(payload, diagnostics)
        captures[side] = {
            "records": len(diagnostics),
            "bytes": payload.stat().st_size,
            "sha256": digest(payload.read_bytes()),
        }
        exe = suite / f"{side}-ty"
        log = shlex.quote(str(suite / f"{side}.argv"))
        exe.write_text(
            "#!/bin/sh\n"
            f'if [ "${{RELAR_BENCH_AUDIT:-}}" = 1 ]; then printf \'%s\\0\' "$PWD" "$@" > {log}; fi\n'
            f"exec /bin/cat {shlex.quote(str(payload))}\n"
        )
        exe.chmod(0o755)
        children.append(exe)
    return (
        suite,
        children,
        {
            "files": len(sources["test_files"]),
            "source_bytes": sum(p.stat().st_size for p in tests.iterdir()),
            "groups_sha256": digest(groups),
            "capture": captures,
        },
    )


def case(commands, suite, children, fmt, cpus, cycles, home, audit):
    native_args = ["--suite", str(suite), "--stdout", "--suite-revision", REVISION]
    original_args = ["--tests-path", str(suite)]
    common = [
        "--old-ty",
        str(children[0]),
        "--new-ty",
        str(children[1]),
        "--format",
        fmt,
    ]
    argv = {
        key: [
            *cmd,
            *(original_args if key in ("relar", "cpython") else native_args),
            *common,
        ]
        for key, cmd in commands.items()
    }
    env = {
        **environment(home),
        "CONFORMANCE_SUITE_COMMIT": REVISION,
        "RAYON_NUM_THREADS": str(len(cpus)),
    }
    os.sched_setaffinity(0, cpus)
    expected = subprocess.run(
        argv["cpython"],
        cwd=suite,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=True,
        timeout=180,
    )
    assert not expected.stderr
    arg_digest = {}
    if audit:
        for key, cmd in argv.items():
            for role in ("old", "new"):
                (suite / f"{role}.argv").unlink(missing_ok=True)
            checked = subprocess.run(
                cmd,
                cwd=suite,
                env={**env, "RELAR_BENCH_AUDIT": "1"},
                stdin=subprocess.DEVNULL,
                capture_output=True,
                check=True,
                timeout=180,
            )
            if checked.stdout != expected.stdout or checked.stderr:
                raise ValueError(
                    f"{key} failed stdout/stderr parity for {suite.name} {fmt}"
                )
            for role in ("old", "new"):
                args = (suite / f"{role}.argv").read_bytes()
                if args.split(b"\0", 1)[0] != os.fsencode(
                    suite
                ) or arg_digest.setdefault(role, digest(args)) != digest(args):
                    raise ValueError(f"{key} changed {role} analyzer cwd or argv")
        assert arg_digest["old"] == arg_digest["new"]
    design = orders(tuple(commands))
    samples = []
    for row in [list(commands), *(design * cycles)]:
        measured = {}
        for key in row:
            before = time.perf_counter_ns()
            result = subprocess.run(
                argv[key],
                cwd=suite,
                env=env,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=180,
            )
            measured[key] = time.perf_counter_ns() - before
            if result.returncode or result.stderr or result.stdout != expected.stdout:
                raise RuntimeError(
                    (
                        key,
                        suite.name,
                        fmt,
                        result.returncode,
                        result.stderr[:500],
                        digest(result.stdout),
                        digest(expected.stdout),
                    )
                )
        samples.append(measured)
    medians = {
        key: statistics.median(row[key] for row in samples[1:]) / 1e6
        for key in commands
    }
    return {
        "median_ms": medians,
        "relar_over_rust": medians["relar"] / medians["rust"],
        "cpython_over_relar": medians["cpython"] / medians["relar"],
        "order": design * cycles,
        "warmup_ns": samples[0],
        "samples_ns": samples[1:],
        "stdout_bytes": len(expected.stdout),
        "stdout_sha256": digest(expected.stdout),
        "analyzer_cwd_and_argv_sha256": arg_digest,
        "commands": argv,
        "cpu": cpus,
        "environment": env,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("fixtures", "checkout", "python", "relar", "codon", "rust", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--cpu", required=True, type=int)
    parser.add_argument("--live", nargs=2, metavar=("OLD_TY", "NEW_TY"), type=Path)
    parser.add_argument("--live-cpus", type=int, nargs="+")
    parser.add_argument("--cycles", type=int, default=2)
    parser.add_argument("--source", type=Path, default=APP / "upstream/conformance.py")
    parser.add_argument(
        "--work-dir", type=Path, help="retain replay checkout at this new path"
    )
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    args = parser.parse_args()
    if bool(args.live) != bool(args.live_cpus) or args.cycles < 1:
        parser.error("supply --live with --live-cpus, and at least one cycle")
    checkout = args.checkout.resolve(strict=True)
    if (
        subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True
        ).strip()
        != REVISION
    ):
        raise ValueError("unexpected typing checkout revision")
    original = args.source.resolve(strict=True)
    if original.read_bytes() != (APP / "upstream/conformance.py").read_bytes():
        raise ValueError("reporter source differs from the pinned Ruff file")
    commands = {"cpython": [str(args.python.absolute()), "-B", str(original)]}
    commands.update(
        {
            key: [str(getattr(args, key).resolve(strict=True))]
            for key in ("codon", "relar", "rust")
        }
    )
    build_records = [
        Path(commands[key][0]).parent / "build-record.json" for key in ("rust", "codon")
    ]
    artifacts = [
        file_id(x)
        for x in (
            original,
            Path(__file__),
            *(Path(c[0]) for c in commands.values()),
            *(args.live or []),
            *(item for item in (args.compiler, args.runtime) if item is not None),
            *(item for item in build_records if item.is_file()),
        )
    ]
    results = {
        "platform": platform.platform(),
        "cpu_model": next(
            line.split(":", 1)[1].strip()
            for line in Path("/proc/cpuinfo").read_text().splitlines()
            if line.startswith("model name")
        ),
        "cpython": subprocess.check_output(
            [commands["cpython"][0], "--version"], text=True
        ).strip(),
        "typing_revision": REVISION,
        "artifacts": artifacts,
        "panels": {},
        "cases": {},
    }
    if args.work_dir is not None:
        args.work_dir = args.work_dir.absolute()
        args.work_dir.mkdir(parents=True, exist_ok=False)
    with (
        nullcontext(args.work_dir)
        if args.work_dir
        else tempfile.TemporaryDirectory(prefix="relar-original-cli-")
    ) as tmp:
        work = Path(tmp)
        for scale in (1, 10):
            suite, tools, inputs = replay(
                args.fixtures.resolve(), checkout, scale, work
            )
            results["panels"][f"{scale}x"] = inputs
            for fmt in ("diff", "github"):
                key = f"replay/{scale}x/{fmt}"
                results["cases"][key] = case(
                    commands, suite, tools, fmt, [args.cpu], args.cycles, work, True
                )
                print(key, results["cases"][key]["median_ms"], flush=True)
        if args.live:
            for fmt in ("diff", "github"):
                key = f"live/1x/{fmt}"
                results["cases"][key] = case(
                    commands,
                    checkout,
                    [p.resolve() for p in args.live],
                    fmt,
                    args.live_cpus,
                    args.cycles,
                    work,
                    False,
                )
                print(key, results["cases"][key]["median_ms"], flush=True)
    args.output.write_bytes(
        gzip.compress(json.dumps(results, sort_keys=True).encode(), mtime=0)
    )


if __name__ == "__main__":
    main()
