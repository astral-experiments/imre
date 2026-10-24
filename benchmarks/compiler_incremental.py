"""Compare actual PR parent/child compiler builds on the unscaled captured Ruff report (Linux)."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import resource
import shutil
import statistics
import subprocess
import sys
from tempfile import TemporaryDirectory
from time import perf_counter_ns, sleep


NAMES = ("parent", "child")
SEED = 78259797
HEADING = b"### Test file breakdown"
ORACLE = "73cbc74bf3f97bfdbbb8933a749156213074ac57ba010d06b75ae48b45a946b2"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def source_hashes(root):
    return {str(f.relative_to(root)): sha(f.read_bytes()) for f in sorted(root.rglob("*.py"))}


def orders(n, seed):
    rng = random.Random(seed)
    sequence = []
    while len(sequence) < n:
        pair = [NAMES, NAMES[::-1]]
        rng.shuffle(pair)
        sequence.extend(pair)
    return sequence[:n]


def descendants(pid):
    try:
        raw = Path(f"/proc/{pid}/task/{pid}/children").read_text()
    except (FileNotFoundError, ProcessLookupError):
        return
    for child in map(int, raw.split()):
        try:
            prefix = Path(f"/proc/{child}")
            exe = os.readlink(prefix / "exe")
            argv = (prefix / "cmdline").read_bytes().rstrip(b"\0").decode().split("\0")
        except (FileNotFoundError, ProcessLookupError):
            continue
        yield exe, tuple(argv)
        yield from descendants(child)


def run(command, env, audit=False):
    start_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = perf_counter_ns()
    proc = subprocess.Popen(command, env=env, stderr=subprocess.PIPE, stdout=subprocess.PIPE)
    observed = set()
    if audit:
        while proc.poll() is None:
            observed.update(descendants(proc.pid))
            sleep(.002)
    stdout, stderr = proc.communicate()
    ms = (perf_counter_ns() - start) / 1_000_000
    end_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    assert not proc.returncode and not stdout, (command, proc.returncode, stdout[:1000], stderr[-12000:])
    return {"ms": ms, "cpu_ms": 1000 * ((end_cpu.ru_utime - start_cpu.ru_utime) +
                                       (end_cpu.ru_stime - start_cpu.ru_stime))}, stderr.decode().splitlines(), observed


def flags_audit(calls, module_objects):
    drivers = [(exe, argv) for exe, argv in calls if "-Wl,--gc-sections" in argv and "-Wl,-z,relro,-z,now" in argv]
    assert len(drivers) == 1, [(exe, argv[:10]) for exe, argv in calls]
    driver, args = drivers[0]
    assert "-O3" in args and any(x.endswith("/libimre_runtime.a") for x in args)
    if module_objects:
        assert sum(opt.endswith(".o") for opt in args) == 7, args
    else:
        assert sum(opt.endswith(".ll") for opt in args) == 1, args
    selected = next((arg.removeprefix("--ld-path=") for arg in args if arg.startswith("--ld-path=")), "/usr/bin/ld")
    ld = [(exe, argv) for exe, argv in calls if Path(exe) == Path(selected).resolve() and "--gc-sections" in argv]
    assert len(ld) == 1
    ld_path, ld_args = ld[0]
    assert any(ld_args[i:i+4] == ("-z", "relro", "-z", "now") for i in range(len(ld_args)-3)), ld_args
    return {"driver": driver, "driver_argv": args, "linker": ld_path, "linker_argv": ld_args}


def elf(native, temp):
    headers = subprocess.check_output(["readelf", "-lW", native], text=True).splitlines()
    dynamic = subprocess.check_output(["readelf", "-dW", native], text=True).splitlines()
    relro = [s.strip() for s in headers if "GNU_RELRO" in s]
    now = [s.strip() for s in dynamic if "BIND_NOW" in s or ("FLAGS_1" in s and "NOW" in s)]
    assert relro and now
    sections = {}
    for section in ("text", "rodata"):
        path = temp / ("section-" + section)
        subprocess.run(["objcopy", "-O", "binary", "--only-section=." + section, native, path], check=True)
        sections[section] = {"bytes": path.stat().st_size, "sha256": sha(path.read_bytes())}
    stripped = temp / "stripped-reporter"
    subprocess.run(["strip", "--strip-all", "-o", stripped, native], check=True)
    return {"bytes": native.stat().st_size, "stripped_bytes": stripped.stat().st_size,
            "sha256": sha(native.read_bytes()), "sections": sections,
            "gnu_relro": relro, "bind_now": now}


def summarize(rows, cluster=1):
    by_variant = {v: {r["pair"]: r for r in rows if r["variant"] == v} for v in NAMES}
    assert by_variant["parent"].keys() == by_variant["child"].keys()
    result = {}
    for metric in ("ms", "cpu_ms"):
        samples = {v: [by_variant[v][i][metric] for i in sorted(by_variant[v])] for v in NAMES}
        logs = [math.log(c/p) for p, c in zip(samples["parent"], samples["child"])]
        groups = [logs[i:i+cluster] for i in range(0, len(logs), cluster)]
        rng = random.Random(SEED + 70)
        bootstrap = sorted(statistics.median(x for group in rng.choices(groups, k=len(groups)) for x in group)
                           for _ in range(5000))
        ratio = math.exp(statistics.median(logs))
        result[metric] = {"medians": {v: statistics.median(x) for v, x in samples.items()},
                          "paired_child_pct": 100 * (ratio - 1), "paired_speedup": 1 / ratio,
                          "paired_pct_ci95": [100 * math.expm1(bootstrap[i]) for i in (125, 4874)]}
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("parent", "child", "parent-source", "child-source", "output", "summary", "scratch-parent"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--parent-revision", required=True)
    p.add_argument("--child-revision", required=True)
    p.add_argument("--cached-parent", action="store_true", help="Expect the parent to use the native module cache too")
    p.add_argument("--inputs", type=Path, help="Existing official 1x/changed panel (otherwise regenerate from archive)")
    p.add_argument("--cpu", type=int, default=min(os.sched_getaffinity(0)))
    p.add_argument("--edit-pairs", type=int, default=6)
    p.add_argument("--cold-pairs", type=int, default=4)
    p.add_argument("--runtime-pairs", type=int, default=72)
    a = p.parse_args()
    if not __debug__ or any(n < 2 or n % 2 for n in (a.edit_pairs, a.cold_pairs, a.runtime_pairs)) or a.edit_pairs>1000:
        p.error("use even positive pair counts and enable assertions")
    os.sched_setaffinity(0, {a.cpu})
    env = {k: v for k, v in os.environ.items() if k not in ("IMREFLAGS", "IMRE_ENCODED_FLAGS", "IMRE_CACHE_DIR")}
    env["IMRE_CACHE_TRACE"] = "1"
    compilers = {v: getattr(a, v).resolve() for v in NAMES}
    sources = {v: getattr(a, v + "_source").resolve() for v in NAMES}
    report_sources = {v: base / "examples/conformance_report" for v, base in sources.items()}
    digests = source_hashes(report_sources["parent"])
    assert digests == source_hashes(report_sources["child"])
    archives = {v: sha(c.with_name("libimre_runtime.a").read_bytes()) for v, c in compilers.items()}
    assert len(set(archives.values())) == 1
    data = {"revisions": {"parent": a.parent_revision, "child": a.child_revision}, "cpu": a.cpu,
            "cpu_model": next(s.split(":", 1)[1].strip() for s in Path("/proc/cpuinfo").read_text().splitlines()
                              if s.startswith("model name")), "load_start": os.getloadavg(), "seed": SEED,
            "compiler_sha256": {v: sha(c.read_bytes()) for v,c in compilers.items()}, "runtime_archive_sha256": archives,
            "report_source_sha256": digests, "clang": subprocess.check_output(["clang-18","--version"], text=True).splitlines()[0],
            "ld": subprocess.check_output(["ld","--version"], text=True).splitlines()[0], "link_audits": {},
            "cold": [], "runtime": [], "cli": [], "oracle_sha256": {}, "prime": []}
    with TemporaryDirectory(dir=a.scratch_parent, prefix="reporter-stack-") as tmp:
        temp = Path(tmp).resolve()
        panel = a.inputs.resolve() if a.inputs else temp / "inputs/1x/changed"
        if not a.inputs:
            generator = sources["parent"] / "benchmarks/conformance_grouping_inputs.py"
            archive = sources["parent"] / "benchmarks/results/2026-10-22-collector-ownership-study.tar.xz"
            subprocess.run([sys.executable, generator, "--archive", archive, "--scales", "1", "--output", temp / "inputs"],
                           check=True, stdout=subprocess.DEVNULL)
        files = {r: panel / (r + ".json") for r in ("manifest","old","new")}
        m = json.loads(files["manifest"].read_bytes())
        data["inputs"] = {"suite": m["suite_revision"], "files": len(m["test_files"]),
                          "old": len(json.loads(files["old"].read_bytes())), "new": len(json.loads(files["new"].read_bytes())),
                          "sha256": {role: sha(path.read_bytes()) for role, path in files.items()}}
        assert [data["inputs"][k] for k in ("files", "old", "new")] == [146, 1169, 1201]
        case = temp / "examples/conformance_report"
        reference = temp / "upstream-reference"
        for dest in (case, reference):
            shutil.copytree(report_sources["parent"], dest, ignore=shutil.ignore_patterns("__pycache__"))
        render, original = case / "render.py", reference / "upstream/conformance.py"
        templates = {path: path.read_bytes() for path in (render, original)}
        assert all(template.count(HEADING) == 1 for template in templates.values())
        report = temp / "report.md"
        pg = [item for role,path in files.items() for item in ("--" + role, path)] + ["--format", "github", "--output", report]

        def oracle(tag):
            report.unlink(missing_ok=True)
            _, out, _ = run([sys.executable, "-B", reference / "original.py", *pg], env)
            expected = report.read_bytes()
            assert not out and sha(expected) not in data["oracle_sha256"].values()
            data["oracle_sha256"][tag] = sha(expected)
            return expected

        def direct(native, expected):
            report.unlink(missing_ok=True)
            timing, out, _ = run([native, *pg], env)
            actual = report.read_bytes()
            assert not out and actual == expected
            return {**timing, "report_sha256": sha(actual)}

        def cli(variant, mode, cache, native, expected, state, audit=False):
            report.unlink(missing_ok=True)
            cmd = [compilers[variant], mode, case / "__main__.py", "--module-root", case.parent]
            cmd.extend(("--", *pg) if mode == "run" else ("-o", native))
            timing, lines, observed = run(cmd, dict(env, IMRE_CACHE_DIR=str(cache)), audit)
            events = [s.removeprefix("imre cache ") for s in lines if s.startswith("imre cache ")]
            objects = [s.split(" ", 3)[2:] for s in lines if s.startswith("imre object ")]
            module_objects = variant == "child" or a.cached_parent
            if module_objects:
                assert events == (["hit"] if state == "hit" else ["miss", "store"]), lines
                if state == "hit":
                    assert not objects
                else:
                    assert len(objects) == len({obj for _, obj in objects}) == 7
                    misses = {obj for status,obj in objects if status == "miss"}
                    assert misses == ({"conformance_report.render"} if state == "edit" else {obj for _,obj in objects}), objects
            else:
                assert not events and not objects
            if audit:
                data["link_audits"][variant] = flags_audit(observed, module_objects)
            if mode == "build":
                got = direct(native, expected)["report_sha256"]
            else:
                actual = report.read_bytes()
                assert actual == expected
                got = sha(actual)
            return dict(**timing, variant=variant, command=mode, cache_events=events, objects=objects, stderr=lines,
                        report_sha256=got)

        pristine = oracle("pristine")
        assert len(pristine) == 109258 and sha(pristine) == ORACLE
        data["report_bytes"] = len(pristine)
        for v in NAMES:
            cli(v, "build", temp / ("audit-" + v), temp / ("audit-bin-" + v), pristine, "cold", audit=True)
        print("actual default Clang and BFD flags captured for both compilers", flush=True)
        programs = {}
        for i, order in enumerate(orders(a.cold_pairs, SEED + 1)):
            for v in order:
                native = programs[i,v] = temp / f"cold-bin-{i}-{v}"
                row = cli(v, "build", temp / f"cold-cache-{i}-{v}", native, pristine, "cold")
                data["cold"].append(dict(pair=i, binary=elf(native, temp), **row))
                if i == 0:
                    direct(temp / "stripped-reporter", pristine)
            print(f"cold pair {i+1}/{a.cold_pairs} verified", flush=True)
        for i, order in enumerate(orders(a.cold_pairs, SEED + 2)):
            for v in order:
                direct(programs[i,v], pristine)
        layouts = []
        rng = random.Random(SEED+3)
        while len(layouts) < a.runtime_pairs:
            group = list(range(a.cold_pairs))
            rng.shuffle(group)
            layouts.extend(group)
        for i, order in enumerate(orders(a.runtime_pairs, SEED + 4)):
            for v in order:
                data["runtime"].append(dict(pair=i, variant=v, layout=layouts[i], **direct(programs[layouts[i], v], pristine)))
        print(f"{a.runtime_pairs} pristine direct-runtime pairs verified", flush=True)
        caches = {(v,mode): temp / (v + "-cache-" + mode) for v in NAMES for mode in ("build", "run")}
        for (v,mode), cache in caches.items():
            other = "run" if mode == "build" else "build"
            row = cli(v, other, cache, temp / "prime-native", pristine, "cold")
            data["prime"].append(dict(for_mode=mode, **row))
        schedule = {(sc,mode): orders(a.edit_pairs, SEED+10+2*i+j) for i,sc in enumerate(("unchanged", "edit"))
                    for j,mode in enumerate(("build", "run"))}
        expected = pristine
        for i in range(a.edit_pairs):
            for scenario in ("unchanged", "edit"):
                if scenario == "edit":
                    label = f"### Test file table {i:03d}".encode()
                    assert len(label) == len(HEADING)
                    for path, template in templates.items():
                        path.write_bytes(template.replace(HEADING, label))
                    expected = oracle(str(i))
                    assert expected == pristine.replace(HEADING, label)
                for mode in (("build","run") if i%2 == 0 else ("run","build")):
                    for v in schedule[scenario,mode][i]:
                        row = cli(v, mode, caches[v,mode], temp / (v + "-reporter"), expected,
                                  "hit" if scenario=="unchanged" else "edit")
                        data["cli"].append(dict(pair=i, scenario=scenario, **row))
            print(f"edit pair {i+1}/{a.edit_pairs} verified", flush=True)
        assert data["inputs"]["sha256"] == {role: sha(path.read_bytes()) for role, path in files.items()}
    assert all(source_hashes(report_sources[v]) == digests for v in NAMES)
    assert data["compiler_sha256"] == {v: sha(c.read_bytes()) for v,c in compilers.items()}
    data["load_end"] = os.getloadavg()
    a.output.write_text(json.dumps(data, indent=2)+"\n")
    results = {"cold_build": summarize(data["cold"]), "direct_runtime": summarize(data["runtime"], cluster=2)}
    for scenario in ("unchanged","edit"):
        for mode in ("build","run"):
            results[scenario + "_" + mode] = summarize([r for r in data["cli"] if r["scenario"] == scenario and r["command"] == mode])
    results["binary_bytes"] = {v: {key: sorted({r["binary"][key] for r in data["cold"] if r["variant"] == v})
                                   for key in ("bytes", "stripped_bytes")} for v in NAMES}
    results["recorded_native_outputs_checked"] = sum(len(data[k]) for k in ("prime","cold","cli","runtime"))
    a.summary.write_text(json.dumps(results, indent=2)+"\n")
    for name, value in results.items():
        print(name, value["ms"] if isinstance(value, dict) and "ms" in value else value)


if __name__ == "__main__":
    main()
