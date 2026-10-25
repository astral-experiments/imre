"""Time Ruff's original ecosystem CLI on the same cached revisions as CPython."""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def git(directory, *arguments):
    return subprocess.check_output(["git", "-C", directory, *arguments], text=True).strip()


def freeze(sources, out):
    """Keep all remote refs fixed, using disposable checkouts with local origins."""
    mirrors = out / "upstreams"
    checkouts = out / "checkouts"
    mirrors.mkdir(parents=True, exist_ok=True)
    checkouts.mkdir(exist_ok=True)

    def prepare(source):
        revision = git(source, "rev-parse", "HEAD")
        mirror = mirrors / (source.name + ".git")
        if not mirror.exists():
            subprocess.run(["git", "clone", "--quiet", "--bare", "--shared", source, mirror], check=True)
        if git(mirror, "rev-parse", "HEAD") != revision:
            raise ValueError(f"{source}: revision differs from the frozen upstream")
        checkout = checkouts / source.name
        if not checkout.exists():
            subprocess.run(["git", "clone", "--quiet", "--shared", mirror, checkout], check=True)
        if git(checkout, "remote", "get-url", "origin") != str(mirror):
            raise ValueError(f"{checkout}: expected a local frozen origin")
        return source.name, revision

    projects = [p for p in sorted(sources.iterdir()) if (p / ".git").is_dir()]
    if not projects:
        raise ValueError("the source checkout cache is empty")
    with ThreadPoolExecutor(max_workers=4) as workers:
        return checkouts, dict(workers.map(prepare, projects))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--python", type=Path, default=Path(sys.executable))
    p.add_argument("--imre", type=Path)
    p.add_argument("--old-ruff", type=Path, required=True)
    p.add_argument("--new-ruff", type=Path, required=True)
    p.add_argument("--cache", type=Path, required=True)
    p.add_argument("--output-dir", type=Path, required=True)
    p.add_argument("--cpython-only", action="store_true")
    p.add_argument("--command", choices=["check", "format"], default="check")
    p.add_argument("--capture-format", type=Path, help="freeze the first CPython format run for replay")
    p.add_argument("--force-preview", action="store_true", help="use Ruff preview rules on every project")
    p.add_argument("--rounds", type=int, default=1)
    p.add_argument("--cpu", type=int, help="Linux CPU for a deterministic Ruff file walk")
    args = p.parse_args()
    if args.rounds < 1:
        p.error("--rounds must be at least one")
    if args.capture_format and args.command != "format":
        p.error("--capture-format requires --command format")
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    cache, revisions = freeze(args.cache.resolve(), out)
    (out / "revisions.json").write_text(json.dumps(revisions, indent=2) + "\n")
    if args.command == "format":
        # Ruff sets this identity when it creates a clone. These disposable
        # clones already exist, so its clone path won't configure them.
        for name in revisions:
            git(cache / name, "config", "user.name", "Ecosystem Bot")
            git(cache / name, "config", "user.email", "ecosystem@astral.sh")
            git(cache / name, "config", "commit.gpgSign", "false")
    if args.cpu is not None:
        if not hasattr(os, "sched_getaffinity") or args.cpu not in os.sched_getaffinity(0):
            p.error("--cpu must select an available Linux CPU")
        os.sched_setaffinity(0, {args.cpu})
    packages = Path(__file__).resolve().parents[1] / "examples" / "ruff_ecosystem"
    env = dict(os.environ, PYTHONPATH=str(packages), PYTHONHASHSEED="0", RAYON_NUM_THREADS="1" if args.cpu is not None else "2")
    command = ["-v", "--cache", cache]
    if args.force_preview:
        command.append("--force-preview")
    command.extend([args.command, args.old_ruff.resolve(), args.new_ruff.resolve()])
    arms = {"cpython": [args.python.absolute(), "-B", "-m", "ruff_ecosystem"]}
    if not args.cpython_only:
        if args.imre is None:
            p.error("--imre is required to compare both engines")
        arms["imre"] = [args.imre.resolve()]
    samples = {arm: [] for arm in arms}
    for round_ in range(args.rounds):
        outputs = {}
        for arm in list(arms)[::1 if round_ % 2 == 0 else -1]:
            report = out / f"{arm}-{round_ + 1}.md"
            launch = arms[arm]
            if args.capture_format and arm == "cpython" and round_ == 0:
                launch = [args.python.absolute(), "-B", Path(__file__).with_name("ruff_ecosystem_format_capture.py"),
                          args.capture_format.resolve(), out / "revisions.json"]
            with report.open("wb") as stdout, (out / f"{arm}-{round_ + 1}.stderr").open("wb") as stderr:
                started = time.perf_counter()
                subprocess.run([*launch, *command], stdout=stdout, stderr=stderr, check=True, env=env)
                seconds = time.perf_counter() - started
            data = outputs[arm] = report.read_bytes()
            sample = {"wall_seconds": seconds, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            samples[arm].append(sample)
            print(round_ + 1, arm, sample, flush=True)
            for project, revision in revisions.items():
                if git(cache / project, "rev-parse", "HEAD") != revision:
                    raise ValueError(f"{project} moved during {arm}")
        if "imre" in outputs and outputs["imre"] != outputs["cpython"]:
            raise ValueError(f"live reports differ in round {round_ + 1}; reports and stderr were saved to {out}")
    evidence = {"command": args.command, "cpu": args.cpu, "force_preview": args.force_preview, "samples": samples, "revisions": revisions, "ruff": [
        subprocess.check_output([exe, "--version"], text=True).strip()
        for exe in (args.old_ruff, args.new_ruff)
    ]}
    (out / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
