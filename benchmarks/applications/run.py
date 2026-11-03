"""Time application CLIs on one quiet Linux CPU and check their complete outputs."""

import argparse
import hashlib
import json
import os
import platform
import runpy
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from zipfile import ZipFile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "benchmarks"))
import conformance_grouping_inputs as captures
import conformance_validate

RUFF = "370f735ce20e5c79777e773249753e006cb2e04e"
PIP = "99cb1c2899893b075ff56e4ed0af55669a955b49ad7fb8d8603ecdaf4ed653fb"
GRAPH = "37f5b3eff82330850466956b6c036a8ab8551512bdc7509edca1d4c6cf943fa3"
SEARCH = "8119d1d18f39ed6ae1de612a0b9b1799e7f8ac1e63d487b221e59b162e40a941"
QUERIES = ["1,12637", "120000,230000", "264346,4321", "63789,130000"]
# Independently computed from the reverse graph using CPython's heapq.
DISTANCES = b"556089\n664234\n408822\n306513\n"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(ok, message):
    if not ok:
        raise ValueError(message)


@dataclass
class Case:
    name: str
    size: str
    commands: dict
    cwd: Path
    expected: bytes
    output: Path | None = None
    unavailable: dict | None = None


def cpu_ticks():
    result = {}
    for line in Path("/proc/stat").read_text().splitlines():
        name, *values = line.split()
        if name.startswith("cpu"):
            result[name] = list(map(int, values[:8]))
    return result


def activity(before, after, cpu):
    def counts(keys):
        ticks = [sum(after[key][i] - before[key][i] for key in keys) for i in range(8)]
        total = max(1, sum(ticks))
        return {
            "busy_pct": round(100 * (total - ticks[3] - ticks[4]) / total, 2),
            "steal_pct": round(100 * ticks[7] / total, 2),
        }

    chosen = f"cpu{cpu}"
    peers = [k for k in before if k not in ("cpu", chosen)]
    return {"selected": counts([chosen]), "other_cpus": counts(peers)}


def idle_window(cpu):
    start = cpu_ticks()
    time.sleep(3)
    return activity(start, cpu_ticks(), cpu)


def idle_enough(window):
    return window["selected"]["busy_pct"] <= 5 and window["other_cpus"]["busy_pct"] <= 2


def wait_for_idle(cpu):
    for _ in range(20):
        window = idle_window(cpu)
        if idle_enough(window):
            return window
    raise RuntimeError(f"host remained busy for one minute: {window}")


def search_fixture(archive, work):
    directory = work / "ruff"
    directory.mkdir()
    paths = []
    combined = hashlib.sha256()
    with tarfile.open(archive) as tar:
        for item in sorted(tar.getmembers(), key=lambda m: m.name):
            if not item.isfile() or not item.name.endswith(".rs"):
                continue
            prefix, rel = item.name.split("/", 1)
            require(
                prefix == "ruff-" + RUFF
                and not Path(rel).is_absolute()
                and ".." not in Path(rel).parts,
                item.name,
            )
            data = tar.extractfile(item).read()
            paths.append(rel)
            combined.update(rel.encode() + b"\0" + hashlib.sha256(data).digest())
            dst = directory / rel
            dst.parent.mkdir(exist_ok=True, parents=True)
            dst.write_bytes(data)
    require(
        combined.hexdigest()
        == "f39b8ef1238ff6ba541379fa4e1f87a1c88ccfcb33fdfb19c494b37c271391cc",
        "Ruff corpus differs",
    )
    manifest = work / "manifest"
    manifest.write_text("\n".join(paths) + "\n")
    # Compare with the digest of git grep -n -F TODO on the pinned tree.
    expected = b"".join(
        f"{path}:{i}\n".encode()
        for path in paths
        for i, line in enumerate((directory / path).read_bytes().split(b"\n"), 1)
        if b"TODO" in line
    )
    require(digest(expected) == SEARCH, "git grep oracle differs")
    return directory, manifest, expected


def prepare(a, work, env):
    build, py = a.build_dir, sys.executable
    rust = build / "target/release"
    examples = ROOT / "examples"
    cases = []

    def commands(native, rs, python, args):
        return {
            k: [*cmd, *args]
            for k, cmd in {
                "imre": [build / native],
                "rust": [rust / rs],
                "cpython": [py, *python],
            }.items()
        }

    out = work / "report.txt"
    catalog = examples / "python_catalog"
    source = catalog / "sample/python-build-standalone.ndjson.gz"
    require(
        digest(source.read_bytes())
        == "d3dcc0f4d8ceaa166eb38db9153bea40dab46afd1e984fa3e4970f7942172a3a",
        "catalog fixture differs",
    )
    original = [py, catalog / "upstream_adapter.py", source, out]
    subprocess.run(original, env=env, check=True)
    oracle = out.read_bytes()
    require(
        digest(oracle)
        == "753cd906307c1b4f830e9348703e0110ba38fddd97293780f146f78f0ed9a3f6",
        "uv oracle differs",
    )
    cmds = commands(
        "catalog",
        "python-catalog-reference",
        ["-m", "python_catalog"],
        [source, out],
    )
    cmds["original_cpython"] = original
    cases.append(
        Case("uv download catalog", "58,721 release artifacts", cmds, work, oracle, out)
    )

    tree, manifest, matches = search_fixture(a.ruff_archive, work)
    cmds = commands("search", "source-search", [HERE / "search.py"], [manifest])
    cases.append(Case("Source search", "1,956 Ruff source files", cmds, tree, matches))

    require(digest(a.wheel.read_bytes()) == PIP, "expected pip 26.1.1 wheel")
    with ZipFile(a.wheel) as archive:
        members = len(archive.infolist())
    cmds = commands(
        "wheel",
        "verify-wheel",
        [examples / "wheel_record/validate.py"],
        [a.wheel],
    )
    bad = work / "damaged" / a.wheel.name
    member = runpy.run_path(str(ROOT / "tests/wheel_record.py"))["tamper_real"](
        a.wheel, bad
    )
    unavailable = {}
    for k, cmd in list(cmds.items()):
        valid = subprocess.run(cmd, env=env, capture_output=True, timeout=120)
        error = valid.stderr.decode(errors="replace").splitlines()
        if (
            k == "imre"
            and valid.returncode
            and error[-1:] == ["ModuleNotFoundError: No module named 'zipfile'"]
        ):
            unavailable[k] = {"exit_code": valid.returncode, "error": error[-1]}
            del cmds[k]
            continue
        require(
            valid.returncode == 0 and not valid.stdout and not valid.stderr,
            f"{k} failed the valid wheel: {error}",
        )
        proc = subprocess.run(
            [*cmd[:-1], bad], env=env, capture_output=True, timeout=120, check=False
        )
        require(
            proc.returncode != 0 and member.encode() in proc.stderr,
            f"{k} missed corrupt member {member}",
        )
    cases.append(
        Case(
            "Wheel verification",
            f"pip 26.1.1 · {members} members",
            cmds,
            work,
            b"",
            unavailable=unavailable,
        )
    )

    require(digest(a.graph.read_bytes()) == GRAPH, "DIMACS graph differs")
    tiny = work / "tiny.gr"
    tiny.write_text("p sp 5 5\na 1 2 20\na 1 2 1\na 2 3 2\na 1 3 8\na 3 4 1\n")
    roads = commands("roads", "road-routing", [examples / "road_routing.py"], [])
    for k, cmd in roads.items():
        p = subprocess.run(
            [*cmd, tiny, "1,4", "4,1", "1,1"], env=env, check=True, capture_output=True
        )
        require(p.stdout == b"4\n-1\n0\n", f"{k} mishandled graph edges")
    for label, desc, queries, expected in [
        ("Road graph load", "NYC · 733,846 directed arcs", [], b"264346 733846\n"),
        ("Road routing", "NYC · load and route 4 trips", QUERIES, DISTANCES),
    ]:
        cases.append(
            Case(
                label,
                desc,
                {k: [*v, a.graph, *queries] for k, v in roads.items()},
                work,
                expected,
            )
        )

    archive = ROOT / "benchmarks/results/2026-10-22-collector-ownership-study.tar.xz"
    changed, unchanged, sources = captures.unpack(archive, work / "capture")
    directory = work / "conformance"
    captures.prepare(changed, unchanged, directory, sources, scales=(1,))
    panel = directory / "1x/changed"
    report = conformance_validate.Workload(
        *(panel / f"{name}.json" for name in ("manifest", "old", "new"))
    )
    expected = conformance_validate.oracle(report)
    require(expected.report is not None, expected.error)
    cmds = commands(
        "conformance",
        "conformance-report-reference",
        ["-m", "conformance_report"],
        [*report.arguments(), "--output", out],
    )
    unchanged = directory / "1x/unchanged"
    control = conformance_validate.Workload(
        *(unchanged / f"{name}.json" for name in ("manifest", "old", "new"))
    )
    expected_control = conformance_validate.oracle(control)
    require(expected_control.report is not None, expected_control.error)
    for engine, cmd in commands(
        "conformance",
        "conformance-report-reference",
        ["-m", "conformance_report"],
        [*control.arguments(), "--output", out],
    ).items():
        out.unlink(missing_ok=True)
        subprocess.run(cmd, cwd=directory, env=env, capture_output=True, check=True)
        require(
            out.read_bytes() == expected_control.report.encode(),
            f"{engine} unchanged report differs",
        )
    cases.append(
        Case(
            "Conformance report",
            "146 files · two captured ty runs",
            cmds,
            directory,
            expected.report.encode(),
            out,
        )
    )
    return cases


def measure(case, env, cpu):
    engines = list(case.commands)
    orders = []
    for sequence in (engines, engines[::-1]):
        orders.extend(sequence[i:] + sequence[:i] for i in range(len(sequence)))
    before = wait_for_idle(cpu)
    start_ticks = cpu_ticks()
    rows = []
    for order in [engines, engines[::-1], *orders, *orders]:
        row = {}
        for engine in order:
            if case.output:
                case.output.unlink(missing_ok=True)
            start = time.perf_counter_ns()
            child = subprocess.run(
                case.commands[engine],
                cwd=case.cwd,
                env=env,
                capture_output=True,
                timeout=150,
                check=False,
            )
            row[engine] = time.perf_counter_ns() - start
            require(
                child.returncode == 0 and not child.stderr,
                f"{engine} {case.name}: {child.stderr.decode(errors='replace')}",
            )
            require(
                child.stdout == (b"" if case.output else case.expected),
                f"{engine} {case.name}: stdout differs",
            )
            if case.output:
                require(
                    case.output.read_bytes() == case.expected,
                    f"{engine} {case.name}: report differs",
                )
        rows.append(row)
    during = activity(start_ticks, cpu_ticks(), cpu)
    return {
        "name": case.name,
        "input": case.size,
        "warmups_ns": rows[:2],
        "samples_ns": rows[2:],
        **({"unavailable": case.unavailable} if case.unavailable else {}),
        "report_bytes": len(case.expected),
        "report_sha256": digest(case.expected),
        "activity_before": before,
        "activity_during": during,
        "activity_after": idle_window(cpu),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ["build-dir", "ruff-archive", "wheel", "graph", "output"]:
        p.add_argument("--" + name, required=True, type=lambda s: Path(s).resolve())
    p.add_argument("--cpu", required=True, type=int)
    p.add_argument(
        "--only", action="append", help="select a workload by its exact display name"
    )
    a = p.parse_args()
    require(not sys.flags.optimize, "suite needs enabled assertions")
    os.sched_setaffinity(0, {a.cpu})
    examples = ROOT / "examples"
    with tempfile.TemporaryDirectory(prefix="imre-applications-") as tmp:
        env = {
            **os.environ,
            "PYTHONPATH": os.pathsep.join(
                map(
                    str, [ROOT / "stdlib", examples, examples / "wheel_record/upstream"]
                )
            ),
            "PYTHONHASHSEED": "0",
            "PYTHONNOUSERSITE": "1",
            "PYTHON_JIT": "0",
            "PYTHONPYCACHEPREFIX": str(Path(tmp) / "pycache"),
            "LC_ALL": "C.UTF-8",
        }
        cases = prepare(a, Path(tmp), env)
        if a.only:
            cases = [case for case in cases if case.name in a.only]
            require(len(cases) == len(set(a.only)), "unknown workload")
        result = {
            "schema": 1,
            "suite": "applications",
            "date_utc": datetime.now(UTC).isoformat(),
            "host": platform.node(),
            "os": platform.platform(),
            "cpu_model": next(
                l.split(":", 1)[1].strip()
                for l in Path("/proc/cpuinfo").read_text().splitlines()
                if l.startswith("model name")
            ),
            "logical_cpu": a.cpu,
            "logical_cpus": os.cpu_count(),
            "load_before": os.getloadavg(),
            "python": sys.version,
            "build": json.loads((a.build_dir / "build.json").read_text()),
            "runner_sha256": digest(Path(__file__).read_bytes()),
            "cases": [],
            "excluded_runs": [],
        }
        a.output.parent.mkdir(parents=True, exist_ok=True)
        partial = a.output.with_suffix(".partial.json")
        for case in cases:
            for attempt in range(1, 5):
                print(f"Measuring {case.name} (attempt {attempt})", flush=True)
                trial = measure(case, env, a.cpu)
                during = trial["activity_during"]
                acceptable = (
                    idle_enough(trial["activity_after"])
                    and during["other_cpus"]["busy_pct"] <= 2
                    and during["selected"]["steal_pct"] <= 0.1
                )
                if acceptable:
                    result["cases"].append(trial)
                else:
                    result["excluded_runs"].append(trial)
                partial.write_text(
                    json.dumps({**result, "complete": False}, indent=2) + "\n"
                )
                if acceptable:
                    break
            else:
                raise RuntimeError(
                    f"host too busy for {case.name}; samples in {partial}"
                )
        result["load_after"] = os.getloadavg()
        result["complete"] = True
        partial.write_text(json.dumps(result, indent=2) + "\n")
        partial.replace(a.output)


if __name__ == "__main__":
    main()
