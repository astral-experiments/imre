"""Materialize distinct conformance files from the pinned suite and captured ty output."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory

ARCHIVE_SHA256 = "921da80b0573387a9e860573808c3d12b2f6cb66e5f061d288e7b09058295c7f"
REVISION = "835b70f3f795f8e061b1f7621178515b0a77a0ad"


def read(path):
    return json.loads(path.read_bytes())


def write(path, value):
    path.write_text(
        json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )


def digest(path):
    return sha256(path.read_bytes()).hexdigest()


def prepare(changed, unchanged, destination, source_directory=None, scales=(1, 10)):
    destination.mkdir(parents=True, exist_ok=False)
    corpus = {}
    counts = {}
    captures = {}
    for state, directory in (("changed", changed), ("unchanged", unchanged)):
        prep = read(directory / "preparation.json")
        manifest = read(directory / "manifest.json")
        assert (
            prep["guards_passed"]
            and prep["suite_revision"] == manifest["suite_revision"] == REVISION
        )
        files = [Path(name) for name in manifest["test_files"]]
        assert len(files) == len({f.name for f in files}) == 146
        for f in files:
            path = source_directory / f.name if source_directory else f
            content = path.read_bytes()
            assert (
                sha256(content).hexdigest()
                == prep["source_sha256"][str(f.relative_to(prep["suite_directory"]))]
            )
            assert corpus.setdefault(f.name, content) == content
        captures[state] = {}
        for side in ("old", "new"):
            assert (
                digest(directory / f"{side}.json")
                == prep["captures"][side]["stdout_sha256"]
            )
            captures[state][side] = read(directory / f"{side}.json")
        if state == "unchanged":
            assert captures[state]["old"] == captures[state]["new"]
        for scale in scales:
            base = destination / f"{scale}x"
            sources = base / "tests"
            sources.mkdir(parents=True, exist_ok=True)
            panel = base / state
            panel.mkdir()
            aliases, names = [], []
            diagnostics = {"old": [], "new": []}
            for copy in range(scale):
                # Original basenames remain intact in the first copy. Every extra file,
                # basename-based tag and diagnostic alias belongs to its own replica.
                prefix = f"copy{copy:02d}_" if copy else ""
                mapping = {str(f): str(sources / (prefix + f.name)) for f in files}
                for f in files:
                    path = Path(mapping[str(f)])
                    if state == "changed":
                        path.write_bytes(corpus[f.name])
                    else:
                        assert path.read_bytes() == corpus[f.name]
                    names.append(str(path))
                raw_paths = {
                    a["raw"]: f"{copy}/{a['raw']}" for a in manifest["diagnostic_paths"]
                }
                aliases.extend(
                    {"raw": raw_paths[a["raw"]], "path": mapping[a["path"]]}
                    for a in manifest["diagnostic_paths"]
                )
                for side in diagnostics:
                    for d in captures[state][side]:
                        original = d["location"]
                        raw = original["path"]
                        # Non-major diagnostics are filtered before path alias lookup.
                        path = raw_paths.get(raw, f"{copy}/{raw}")
                        diagnostics[side].append(
                            {**d, "location": {**original, "path": path}}
                        )
            assert len(names) == len({Path(s).name for s in names}) == scale * 146
            write(
                panel / "manifest.json",
                {
                    "suite_revision": REVISION,
                    "test_files": names,
                    "diagnostic_paths": aliases,
                },
            )
            for side, records in diagnostics.items():
                write(panel / f"{side}.json", records)
            counts[f"{scale}x/{state}"] = {
                "files": len(names),
                "aliases": len(aliases),
                "records": {
                    side: len(records) for side, records in diagnostics.items()
                },
                "inputs": {
                    name: digest(panel / (name + ".json"))
                    for name in ("manifest", "old", "new")
                },
            }
    result = {
        "revision": REVISION,
        "sources": {k: sha256(v).hexdigest() for k, v in sorted(corpus.items())},
        "panels": counts,
    }
    write(destination / "provenance.json", result)
    return result


def unpack(archive, root):
    if digest(archive) != ARCHIVE_SHA256:
        raise ValueError("unexpected conformance study archive digest")
    suffixes = {"manifest.json", "old.json", "new.json", "preparation.json"}
    with tarfile.open(archive) as study:
        for member in study:
            parts = Path(member.name).parts
            if not member.isfile() or len(parts) != 3 or parts[0] != "inputs":
                continue
            group, name = parts[1:]
            if not (
                (group == "suite" and name.endswith((".py", ".pyi")))
                or (
                    group in {"changed-github", "unchanged-github"} and name in suffixes
                )
            ):
                continue
            path = root / group / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(study.extractfile(member).read())
    return root / "changed-github", root / "unchanged-github", root / "suite"


def check_reporter(binary, output, scales):
    oracle = runpy.run_path(str(Path(__file__).with_name("conformance_validate.py")))
    checks = {}
    for scale in scales:
        for state in ("changed", "unchanged"):
            panel = output / f"{scale}x" / state
            for fmt in ("github", "diff"):
                case = oracle["Workload"](
                    *(panel / (role + ".json") for role in ("manifest", "old", "new")),
                    fmt,
                )
                expected = oracle["oracle"](case)
                assert expected.error is None and expected.report is not None
                name = f"{scale}x-{state}-{fmt}"
                dest = output / (name + ".md")
                result = subprocess.run(
                    [binary, *case.arguments(), "--output", str(dest)],
                    capture_output=True,
                    check=True,
                    timeout=120,
                )
                assert (
                    not result.stdout
                    and not result.stderr
                    and dest.read_bytes() == expected.report.encode("utf-8")
                ), name
                checks[name] = {"bytes": dest.stat().st_size, "sha256": digest(dest)}
    write(output / "checked-reports.json", checks)
    return checks


if __name__ == "__main__":
    if sys.flags.optimize:
        raise SystemExit("fixture validation requires assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--archive",
        type=Path,
        help="Pinned collector-ownership study archive from benchmarks/results",
    )
    parser.add_argument("--changed-captures", type=Path)
    parser.add_argument("--unchanged-captures", type=Path)
    parser.add_argument(
        "--source-directory",
        type=Path,
        help="Retained original source files by basename",
    )
    parser.add_argument(
        "--scales", type=int, nargs="+", choices=(1, 10), default=(1, 10)
    )
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--check-reporter",
        type=Path,
        help="Run the native executable against four complete upstream report oracles per scale",
    )
    args = parser.parse_args()
    output = args.output.resolve()
    with TemporaryDirectory() as tmp:
        if args.archive:
            if (
                args.changed_captures
                or args.unchanged_captures
                or args.source_directory
            ):
                parser.error("choose the study archive or standalone captures")
            inputs = unpack(args.archive, Path(tmp))
        else:
            if not (args.changed_captures and args.unchanged_captures):
                parser.error("provide the study archive or both captures")
            inputs = (
                args.changed_captures,
                args.unchanged_captures,
                args.source_directory,
            )
        counts = prepare(*inputs[:2], output, inputs[2], args.scales)["panels"]
    if args.check_reporter:
        print(
            json.dumps(
                check_reporter(args.check_reporter.resolve(), output, args.scales),
                indent=2,
            )
        )
    else:
        print(json.dumps(counts, indent=2))
