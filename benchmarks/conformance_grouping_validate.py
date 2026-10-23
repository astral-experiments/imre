"""Differential tests for ordered grouping, equivalence and per-file statistics."""

import argparse
import json
import random
import re
import runpy
import subprocess
from collections import Counter
from hashlib import sha256
from pathlib import Path

shared = runpy.run_path(str(Path(__file__).with_name("conformance_regex_benchmark.py")))
ENGINES = shared["ENGINES"]
VALIDATION = shared["VALIDATION"]


def fixture(directory, seed, special=None):
    rng = random.Random(seed)
    paths = [
        directory / part / name
        for part, name in [
            ("left", "same.py"),
            ("right", "same.py"),
            ("right", "é.py"),
            ("extra", "empty.py"),
        ]
    ]
    for path in paths:
        path.parent.mkdir(parents=True, exist_ok=True)
    for index, path in enumerate(paths):
        lines = (
            ["pass"]
            if index == 3
            else [
                "x = 1  # E[same+]: first",
                "long = '©'  # E?[same+]: second",
                "pass  # E: required",
                "pass  # E?: optional",
            ]
        )
        if index == 1:
            lines[0] = "pass  # E[other]: competing last tag on the same basename/line"
        if special == "no-annotations":
            lines = ["pass"]
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    raw = {i: f"gitlab/{i}" for i in range(4)}
    aliases = [{"raw": v, "path": str(paths[i])} for i, v in raw.items()]
    aliases.append({"raw": "alias-for-left", "path": str(paths[0])})

    def diagnostic(index, line, column, description, tag="major"):
        return {
            "check_name": "rule",
            "description": description,
            "severity": tag,
            "location": {
                "path": raw[index],
                "positions": {
                    "begin": {"line": line, "column": column},
                    "end": {"line": line + 1, "column": column + 7},
                },
            },
        }

    items = []
    for i in range(rng.randrange(3, 12)):
        index = rng.choice([0, 1, 2])
        line = rng.choice([1, 1, 2, 3, 4, 5, 10])
        column = rng.choice([0, 1, 1, 9])
        description = rng.choice(
            ["rule: first", "rule: escaped\\tab\tline\n", "equal", "δ"]
        )
        items.append(diagnostic(index, line, column, description))
    # Exact duplicate multiset; shuffled order; identical fingerprint with changed end.
    anchor = diagnostic(0, 1, 1, "same begin")
    old = [anchor, dict(anchor), *items, diagnostic(1, 3, 8, "old only")]
    new = [
        *items[::-1],
        anchor,
        {
            **anchor,
            "location": {
                **anchor["location"],
                "path": "alias-for-left",
                "positions": {
                    "begin": anchor["location"]["positions"]["begin"],
                    "end": {"line": 22, "column": 11},
                },
            },
        },
        diagnostic(2, 5, 2, "new only"),
        diagnostic(2, 5, 9, "ignored", "minor"),
    ]
    rng.shuffle(old)
    rng.shuffle(new)
    if special == "reordered":
        old = [diagnostic(0, 1, 1, "first"), diagnostic(0, 1, 9, "last")]
        new = old[::-1]
    if special == "duplicate-fingerprint":
        old = [anchor, dict(anchor), diagnostic(0, 1, 1, "different")]
        new = [
            anchor,
            diagnostic(0, 1, 1, "different"),
            diagnostic(0, 1, 1, "different"),
        ]
    if special == "empty-old":
        old = []
    if special == "empty-new":
        new = []
    if special == "only-expected":
        old = []
        new = []
    if special == "missing-registered-file":
        aliases[0]["path"] = str(directory / "outside.py")
        aliases[-1]["path"] = aliases[0]["path"]
    if special == "unknown-alias":
        old[0]["location"] = {**old[0]["location"], "path": "unknown"}
    if special == "missing-source":
        paths[3].unlink()
    if special == "symlink-dedup":
        # A source without annotations has two declared spellings; the exact statistics
        # function retains one resolved row while the historical string port retained two.
        alias = directory / "empty-alias.py"
        alias.symlink_to(paths[3])
        paths.append(alias)
    if special == "duplicate-alias":
        aliases.append(dict(aliases[0]))
    if special == "reversed-files":
        paths.reverse()
    manifest = {
        "suite_revision": "synthetic",
        "test_files": [str(p) for p in paths],
        "diagnostic_paths": aliases,
    }
    for name, value in [("manifest", manifest), ("old", old), ("new", new)]:
        (directory / (name + ".json")).write_text(
            json.dumps(value, ensure_ascii=False), encoding="utf-8"
        )
    return [str(directory / (k + ".json")) for k in ("manifest", "old", "new")]


def observation(command, args, cwd, env):
    cp = subprocess.run(
        [*command, *args],
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        timeout=60,
        check=False,
    )
    if cp.returncode == 0:
        assert not cp.stderr
        return None, cp.stdout
    stderr = re.sub(rb"\x1b\[[;0-9]*m", b"", cp.stderr)
    kinds = re.findall(
        rb"\b(KeyError|AssertionError|ValueError|FileNotFoundError|OSError)\b",
        stderr,
    )
    assert kinds, (command, cp.returncode, cp.stderr[-1200:])
    return kinds[-1].decode(), b""


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--config", required=True, type=Path)
    ap.add_argument("--output", required=True, type=Path)
    ap.add_argument("--seeds", type=int, default=96)
    ap.add_argument("--start", type=int, default=1729)
    ap.add_argument("--engines", nargs="+", choices=ENGINES, default=ENGINES)
    opts = ap.parse_args()
    root = opts.output.resolve()
    root.mkdir(parents=True)
    cfg = json.loads(opts.config.read_bytes())
    env = VALIDATION["environment"](root)
    modes = ["dump", "aliases"]
    fixed = [
        "empty-old",
        "empty-new",
        "only-expected",
        "missing-registered-file",
        "unknown-alias",
        "missing-source",
        "symlink-dedup",
        "duplicate-alias",
        "no-annotations",
        "reversed-files",
        "reordered",
        "duplicate-fingerprint",
    ]
    failures = {
        "missing-registered-file": "KeyError",
        "unknown-alias": "KeyError",
        "missing-source": "FileNotFoundError",
        "duplicate-alias": "ValueError",
        "no-annotations": "AssertionError",
    }
    rows = []
    for seed, special in [(opts.start, key) for key in fixed] + [
        (i, None) for i in range(opts.start, opts.start + opts.seeds)
    ]:
        d = root / (special or f"seed-{seed}")
        d.mkdir()
        args = fixture(d, seed, special)
        for mode in modes if special not in failures else ["dump"]:
            suffix = [mode, *args, "1"]
            oracle, data = observation(
                cfg["engines"]["upstream"]["analysis"], suffix, d, env
            )
            assert oracle == failures.get(special), (special, oracle)
            fields = (
                Counter(l[:1].decode() for l in data.splitlines())
                if mode == "dump"
                else {}
            )
            if not oracle and mode == "dump":
                assert fields["F"] == 4
                keys = [
                    line.split(b"\t")[1]
                    for line in data.splitlines()
                    if line.startswith(b"G\t")
                ]
                assert keys == sorted(set(keys))
            samples = {}
            for e in opts.engines:
                if e == "before" and special == "symlink-dedup" and mode == "dump":
                    err, observed = observation(
                        cfg["engines"][e]["analysis"], suffix, d, env
                    )
                    assert (
                        err is None
                        and sum(l.startswith(b"F\t") for l in observed.splitlines())
                        == 5
                    )
                    samples[e] = "historical fifth file row"
                else:
                    err, observed = (
                        (oracle, data)
                        if e == "upstream"
                        else observation(cfg["engines"][e]["analysis"], suffix, d, env)
                    )
                    assert (err, observed) == (oracle, data), (
                        special,
                        seed,
                        mode,
                        e,
                        err,
                        oracle,
                        observed[:120],
                        data[:120],
                    )
                    samples[e] = err or sha256(observed).hexdigest()
            rows.append(
                {
                    "case": special or seed,
                    "mode": mode,
                    "error": oracle,
                    "records": dict(fields),
                    "oracle_sha256": sha256(data).hexdigest(),
                    "engines": samples,
                }
            )
    (root / "validation.json").write_text(
        json.dumps(
            {
                "start": opts.start,
                "seeds": opts.seeds,
                "fixed": fixed,
                "comparisons": rows,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"{len(rows)} differential observations across {len(opts.engines)} engines")


if __name__ == "__main__":
    main()
