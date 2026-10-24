"""Complete catalog bytes and filesystem effects against unchanged uv code."""

import argparse
import gzip
import hashlib
import json
import os
import random
import re
import runpy
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions; do not use -O")

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "python_catalog"
ENTRY = PACKAGE / "__main__.py"
STDLIB = ROOT / "stdlib"
files = SimpleNamespace(**runpy.run_path(str(ROOT / "benchmarks/manifest_validate.py")))
Case = files.Case
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
snapshot = files.snapshot
digest = files.digest
BOOTSTRAP = (
    "import runpy, sys; module = sys.argv[1]; sys.path[:0] = sys.argv[2:4]; "
    "sys.argv = [module, *sys.argv[4:]]; "
    "runpy.run_module(module, run_name='__main__', alter_sys=True)"
)
CODON_EXCLUSIONS = {
    "duplicate-known-field": "Codon's native JSON parser keeps the last duplicate field and has no object-pairs hook.",
    "arbitrary-precision-version-and-release": "Codon's native integers are signed 64-bit values; this case requires Python arbitrary-precision integers.",
    "gzip-missing-magic": "Codon's zlib-backed gzopen accepts uncompressed bytes transparently.",
    "gzip-zero-padded-members": "Codon's gzread stops at zero padding before the next member.",
}


def python_command(python, compute=False):
    module = "python_catalog.compute" if compute else "python_catalog"
    return [str(python), "-P", "-s", "-B", "-c", BOOTSTRAP, module, str(PACKAGE_ROOT), str(STDLIB)]


def original_command(python, compute=False):
    return [str(python), "-P", "-s", "-B", str(PACKAGE / "upstream_adapter.py"), *(["--compute"] if compute else [])]


def environment():
    result = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "0", "PYTHONPATH": "",
              "CODON_PYTHON": "/nonexistent/imre-native-codon-libpython.so"}
    result.pop("PYTHONHOME", None)
    return result


def verify_provenance():
    source = json.loads((PACKAGE / "upstream/UPSTREAM.json").read_text())
    for name, expected in source["files"].items():
        assert digest((PACKAGE / "upstream" / name).read_bytes()) == expected, name
    provenance = json.loads((PACKAGE / "sample/provenance.json").read_text())
    packed = (PACKAGE / "sample/python-build-standalone.ndjson.gz").read_bytes()
    assert len(packed) == provenance["gzip_bytes"] and digest(packed) == provenance["gzip_sha256"]
    raw = gzip.decompress(packed)
    assert len(raw) == provenance["bytes"] and digest(raw) == provenance["sha256"]
    assert hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest() == provenance["git_blob"]
    records = [json.loads(line) for line in raw.splitlines() if line.strip()]
    assert len(records) == provenance["release_rows"]
    assert sum(len(record["artifacts"]) for record in records) == provenance["raw_artifacts"]
    return raw


def artifact(name="one", platform="x86_64-unknown-linux-gnu", variant="install_only_stripped", sha256="a" * 64):
    return {"url": "https://example.invalid/" + name + ".tar.gz", "platform": platform,
            "variant": variant, "sha256": sha256}


def release(artifacts, version="3.14.0+20261020"):
    return {"version": version, "artifacts": artifacts}


def ndjson(records):
    return ("".join(json.dumps(record, ensure_ascii=True) + "\n" for record in records)).encode()


def member_coverage_rows():
    """A separate winner in every member makes premature EOF observable."""
    return [ndjson([release([artifact(f"member-{i}")], f"3.14.{i}+20261020")]) for i in range(10)]


def bounded_cache_members():
    """Fill the target cache with distinct vendors that normalize to the same Linux target."""
    aliases = [artifact("primary", platform=f"x86_64-v{i}-linux-gnu", variant="full") for i in range(520)]
    return [
        ndjson([release(aliases)]),
        ndjson([release([artifact("power", platform="ppc64le-ibm-linux-gnu", variant="install_only")])]),
        ndjson([
            release([artifact("primary", platform="x86_64-v519-linux-gnu", sha256="1" * 64),
                     artifact("power", platform="powerpc64le-unknown-linux-gnu", sha256="2" * 64)]),
            # The same cached and uncached targets carry new per-release checksums.
            release([{**aliases[0], "sha256": "3" * 64}], "3.15.0+20261020"),
            release([{**aliases[-1], "sha256": "4" * 64}], "3.13.0+20261020"),
        ]),
    ]


def case(label, records, output="catalog.json"):
    raw = records if isinstance(records, bytes) else ndjson(records)
    initial = {"index.ndjson": raw, "catalog.json": b"preserve the previous complete catalog\n"}
    return Case(label, initial, ("index.ndjson", output))


def cases(include_real=True):
    result = []

    def add(value, error=None, original=True):
        result.append((value, error, original))

    if include_real:
        add(case("real-complete-cpython-index", verify_provenance()))
    def compressed(label, data, error=None):
        add(Case(label, {"index.ndjson.gz": data, "catalog.json": b"previous catalog\n"},
                 ("index.ndjson.gz", "catalog.json")), error)

    rows = member_coverage_rows()
    members = [gzip.compress(row, mtime=0) for row in rows]
    compressed("gzip-ten-distinct-members", b"".join(members))
    compressed("gzip-member-splits-a-record", gzip.compress(rows[0][:32], mtime=0)
               + gzip.compress(rows[0][32:], mtime=0))
    compressed("gzip-empty-file", b"")
    compressed("gzip-zero-padded-members", members[0] + bytes(7) + members[1] + bytes(7))
    compressed("gzip-missing-magic", rows[0], "BadGzipFile")
    compressed("gzip-truncated-last-member", b"".join(members)[:-3], "EOFError")
    damaged = bytearray(members[-1])
    damaged[-8] ^= 0xff
    compressed("gzip-corrupt-last-member-checksum", b"".join(members[:-1]) + damaged, "BadGzipFile")
    malformed, padding, invalid = b"{broken}\n", b" \n" * 6000, b"\xff\n"
    add(case("malformed-row-before-invalid-utf8", malformed + padding + invalid), "UnicodeDecodeError")
    compressed("gzip-malformed-row-before-invalid-utf8", gzip.compress(malformed + padding + invalid, mtime=0),
               "UnicodeDecodeError")
    for label, first in (("malformed-row", malformed), ("invalid-utf8", invalid)):
        compressed("gzip-" + label + "-before-incorrect-crc", gzip.compress(first + padding, mtime=0) + damaged,
                   "BadGzipFile")
    add(case("empty-index", b"\n \t\r\n\x1c\x85".decode("latin1").encode()))
    add(case("single-release", [release([artifact()])]))
    add(case("flavor-preference-before-release-date", [
        release([artifact("old-stripped", variant="install_only_stripped")], "3.14.0+20250311"),
        release([artifact("new-install", variant="install_only")], "3.14.0+20261020"),
    ]))
    add(case("equal-priority-keeps-first-release", [
        release([artifact("first")], "3.14.0+20250311"),
        release([artifact("second")], "3.14.0+20261020"),
    ]))
    add(case("url-sort-and-stable-duplicate-ties", [release([
        artifact("z", sha256="last"), artifact("a", sha256="first"), artifact("a", sha256="second"),
    ])]))
    preferences = ["install_only_stripped", "install_only", "shared-pgo", "shared-noopt", "static-noopt", "unknown"]
    add(case("all-flavor-priority-pairs", [release([
        artifact("left", variant=left), artifact("right", variant=right),
    ], f"3.14.{left_index * len(preferences) + right_index}+20261020")
        for left_index, left in enumerate(preferences) for right_index, right in enumerate(preferences)]))
    add(case("optimization-options-and-static-spelling", [release([
        artifact("a-noopt", variant="noopt+full"), artifact("b-lto", variant="lto+full"),
        artifact("c-pgo", variant="pgo+full"), artifact("d-both", variant="lto+pgo+full"),
        artifact("e-static", variant="static+pgo+lto+full"),
    ]), release([artifact("legacy-static-noopt", variant="static-noopt+full")], "3.13.0+20261020"),
        release([artifact("literal-static-flavor", variant="static")], "3.12.0+20261020")]))
    add(case("musl-cutoff-and-static-filter", [
        release([artifact("before", platform="x86_64-unknown-linux-musl")], "3.14.0+20250310"),
        release([artifact("at", platform="x86_64-unknown-linux-musl")], "3.14.0+20250311"),
        release([artifact("static-invalid-triple", platform="invalid", variant="static+full")]),
    ]))
    versions = ["3.7.99", "3.8.0", "3.14.0a2", "3.14.0a10", "3.14.0b1", "3.14.0rc2", "3.14.0rc10", "3.14.0", "3.15.0a1", "4.0.0", "2.99.0"]
    add(case("version-prerelease-and-supported-floor", [release([artifact(str(index))], version + "+20261020")
                                                          for index, version in enumerate(versions)]))
    add(case("normalized-version-groups", [release([artifact("first")], "03.014.00+020261020"),
                                            release([artifact("second")], "3.14.0+20261021")]))
    cache_members = bounded_cache_members()
    add(case("bounded-target-cache", b"".join(cache_members)))
    compressed("gzip-bounded-target-cache", b"".join(gzip.compress(data, mtime=0) for data in cache_members))
    add(case("stable-numeric-prerelease-ties", [
        release([artifact("filtered", variant="static+full")], "3.14.0a01+20261020"),
        release([artifact("first")], "3.14.0a1+20261020"),
        release([artifact("second")], "3.14.0a01+20261020"),
    ]))
    platforms = ["ppc64-unknown-linux-gnu", "ppc64le-unknown-linux-gnu", "x86_64_v4-extra-linux-musl",
                 "x86_64_v2_ignored-extra-linux-gnu", "x86_64_v2-extra-linux-gnu", "x86_64-pc-windows-msvc-extra",
                 "aarch64-apple-darwin", "riscv64-unknown-linux-gnu", "unknown", "x86_64-unknown-linux"]
    add(case("architecture-aliases-and-triple-normalization", [release([artifact(str(index), platform=value)
                                                                         for index, value in enumerate(platforms)])]))
    suffixes = ["", "-debug", "-freethreaded", "-freethreaded-debug", "-debug-freethreaded", "-lto", "-pgo"]
    add(case("platform-option-suffix-order", [release([artifact(str(index), platform="aarch64-apple-darwin" + suffix)],
                                                      f"3.14.{index}+20261020") for index, suffix in enumerate(suffixes)]))
    variants = ["full", "debug+full", "freethreaded+full", "debug+freethreaded+full", "shared-freethreaded+pgo+full",
                "debug+unknown", "lto+install_only", "pgo+install_only_stripped"]
    add(case("build-variant-classification-and-order", [release([artifact(str(index), variant=value)
                                                                for index, value in enumerate(variants)])]))
    missing = artifact("missing")
    del missing["sha256"]
    add(case("optional-checksums-and-ignored-fields", [
        {**release([missing, {**artifact("null", platform="aarch64-apple-darwin", sha256=None), "ignored": [1, True, {"x": "y"}]}]), "date": "ignored"},
        release([artifact("empty", sha256="")], "3.13.0+20261020"),
    ]))
    add(case("unicode-and-json-escaping", [release([
        artifact('quote"backslash\\tab\tcontrol\x7f-Ω-🙂-%FF-%E2%82%AC-+', platform="arch-unknown-Ω"),
        artifact("null-sha", platform="arch-unknown-🙂", sha256=None),
    ])]))
    add(case("unicode-decimal-and-underscores", [release([artifact()], "٣.١٤.٠+２０２６１０２０"),
                                                  release([artifact("next")], "3.1_5.0a1_0+20_261_020")]))
    add(case("arbitrary-precision-version-and-release", [
        release([artifact("wide")], f"{10**40}.14.0+{10**50}"),
        release([artifact("wide-prerelease")], f"3.14.0rc{10**40}+20261020"),
        release([artifact("negative")], f"-{10**40}.0.0+20261020"),
    ]))
    add(case("ignored-invalid-prerelease-after-static-filter", [release([
        artifact("skip", variant="static+full"),
    ], "3.14.0ax+20261020")]))
    add(case("ignored-empty-arch-variant-after-static-filter", [release([
        artifact("skip", platform="x86_64_-unknown-linux-gnu", variant="static+full"),
    ])]))
    base = ndjson([release([artifact()])])
    add(case("utf8-bom-is-not-json", b"\xef\xbb\xbf" + base), "ValueError")
    add(case("crlf-only-boundaries", b"\r" + base.replace(b"\n", b"\r\n") + b"\r\n" + base + b"\r"))
    add(case("mixed-line-boundaries", "\u2003\x1f".encode() + base.rstrip() + "\u2028\u0085\r\n".encode()))
    line_boundaries = ["\n", "\r", "\r\n", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"]
    add(case("every-python-line-boundary", b"".join(
        ndjson([release([artifact(str(index))], f"3.14.{index}+20261020")]).rstrip() + boundary.encode()
        for index, boundary in enumerate(line_boundaries)
    )))
    add(case("duplicate-unknown-fields", base.rstrip()[:-1] + b',"ignored":1,"ignored":2}\n'))
    randomizer = random.Random(20261021)
    for index in range(8):
        records = []
        for _ in range(12):
            records.append(release([artifact(
                str(randomizer.randrange(8)), platform=randomizer.choice(platforms),
                variant=randomizer.choice(variants + ["static+full", "pgo+lto+full"]),
                sha256=str(randomizer.randrange(5)),
            ) for _ in range(18)], randomizer.choice(versions) + "+" + randomizer.choice(["20250310", "20250311", "20261020"])))
        add(case("mixed-selection-" + str(index), records))
    for label, raw in [
        ("malformed-json-after-valid-row", base + b"{\n"),
        ("invalid-version-components", ndjson([release([], "3.14+20261020")])),
        ("extra-version-components", ndjson([release([], "3.14.0.1+20261020")])),
        ("invalid-release-date", ndjson([release([], "3.14.0+not-a-date")])),
        ("invalid-selected-prerelease", ndjson([release([artifact()], "3.14.0ax+20261020")])),
    ]:
        add(case(label, raw), "ValueError")
    for label, raw in [
        ("duplicate-known-field", base.rstrip()[:-1] + b',"version":"3.14.1+20261020"}'),
        ("missing-required-field", b'{"version":"3.14.0+20261020"}'),
        ("wrong-field-type", base.replace(b'"artifacts": [', b'"artifacts": null, "ignored": [', 1)),
        ("known-unpaired-surrogate", base.replace(b'"sha256": "' + b'a' * 64 + b'"', b'"sha256": "\\ud800"')),
        ("empty-architecture-variant", ndjson([release([artifact(platform="x86_64_-unknown-linux-gnu")])])),
    ]:
        add(case(label, raw), "ValueError", False)
    add(case("invalid-input-utf8", b"\xff"), "UnicodeDecodeError")
    add(case("create-output-parents", [release([artifact()])], "nested/output/catalog.json"))
    add(case("output-overwrites-input", [release([artifact()])], "index.ndjson"))
    add(Case("output-hardlink", {"index.ndjson": base}, ("index.ndjson", "catalog.json"),
             hardlinks={"catalog.json": "index.ndjson"}))
    add(Case("missing-input", {"catalog.json": b"keep"}, ("index.ndjson", "catalog.json")), "FileNotFoundError")
    add(Case("output-is-directory", {"index.ndjson": base}, ("index.ndjson", "catalog.json"), ("catalog.json",)), "IsADirectoryError")
    add(Case("output-parent-is-file", {"index.ndjson": base, "blocked": b"keep"},
             ("index.ndjson", "blocked/catalog.json")), "FileExistsError")
    add(Case("normalized-paths", {"index.ndjson": base}, ("./index.ndjson//", "./catalog.json//")))
    add(Case("bad-argument-count", {"catalog.json": b"keep"}, ()), "ValueError")
    assert len({value.label for value, _, _ in result}) == len(result)
    return result


def oracle(value, error=None, original=True):
    with tempfile.TemporaryDirectory(prefix="catalog-oracle-") as temporary:
        directory = Path(temporary)
        materialize(value, directory)
        actual_error = None
        if original:
            adapter = runpy.run_path(str(PACKAGE / "upstream_adapter.py"))
            try:
                if len(value.arguments) != 2:
                    raise ValueError("expected INPUT.ndjson OUTPUT.json")
                text = adapter["read_source"](directory / value.arguments[0])
                adapter["run"](text, directory / value.arguments[1])
            except Exception as caught:
                actual_error = "ValueError" if isinstance(caught, ValueError) and not isinstance(caught, UnicodeError) else type(caught).__name__
            assert actual_error == error, (value.label, actual_error, error)
        else:
            assert error == "ValueError", "bounded schema rejections happen before output creation"
        contents, directories = snapshot(directory)
        return {"error": error, "files": contents, "directories": directories}


def compute_case(value, rounds):
    return Case(value.label + "-compute-" + str(rounds), value.files,
                (str(rounds), *value.arguments), value.directories,
                hardlinks=value.hardlinks)


def compute_cases(include_real=True):
    by_name = {value.label: (value, error, original) for value, error, original in cases(include_real)}
    result = []
    selected = [("single-release", 3), ("unicode-and-json-escaping", 2),
                ("normalized-paths", 2), ("output-overwrites-input", 2),
                ("output-parent-is-file", 2), ("missing-input", 0),
                ("malformed-json-after-valid-row", 2), ("duplicate-known-field", 2),
                ("arbitrary-precision-version-and-release", 2)]
    if include_real:
        selected[:0] = [("real-complete-cpython-index", 0), ("real-complete-cpython-index", 2)]
    for name, rounds in selected:
        value, error, original = by_name[name]
        result.append((compute_case(value, rounds), error, original))
    malformed = by_name["malformed-json-after-valid-row"][0]
    result.append((compute_case(malformed, 0), None, True))
    missing = by_name["missing-input"][0]
    result.append((compute_case(missing, -1), "ValueError", True))
    result.append((Case("compute-bad-argument-count", {"catalog.json": b"keep"}, ()), "ValueError", True))
    return result


def compute_oracle(value, error=None, original=True):
    with tempfile.TemporaryDirectory(prefix="catalog-compute-oracle-") as temporary:
        directory = Path(temporary)
        materialize(value, directory)
        stdout = ""
        actual_error = None
        if original:
            adapter = runpy.run_path(str(PACKAGE / "upstream_adapter.py"))
            try:
                if len(value.arguments) != 3:
                    raise ValueError("expected ROUNDS INPUT.ndjson OUTPUT.json")
                rounds, source, output = value.arguments
                checksum = adapter["compute"](int(rounds), directory / source, directory / output)
                stdout = str(checksum) + "\n"
            except Exception as caught:
                actual_error = "ValueError" if isinstance(caught, ValueError) and not isinstance(caught, UnicodeError) else type(caught).__name__
            assert actual_error == error, (value.label, actual_error, error)
        else:
            assert error == "ValueError"
        contents, directories = snapshot(directory)
        return {"error": error, "stdout": stdout, "files": contents, "directories": directories}


def check_result(value, expected, process, directory, engine="port"):
    actual, directories = snapshot(directory)
    stdout, stderr = process.stdout.decode(), process.stderr.decode()
    assert stdout == expected.get("stdout", ""), (value.label, engine, stdout)
    if expected["error"]:
        allowed = {"ValueError", "JSONDecodeError"} if expected["error"] == "ValueError" else {expected["error"]}
        if engine == "codon" and value.label.startswith("gzip-") and expected["error"] in ("EOFError", "BadGzipFile"):
            # Codon's gzread/gzclose surfaces these zlib errors as OSError.
            allowed.add("OSError")
        # Codon's native exception printer adds ANSI colors even when captured.
        plain_stderr = re.sub(r"\x1b\[[0-9;]*m", "", stderr)
        categories = [name for name in re.findall(r"^([A-Za-z_][\w.]*):", plain_stderr, re.MULTILINE)
                      if name.endswith(("Error", "Exception", "BadGzipFile"))]
        assert process.returncode != 0 and categories and categories[-1].rsplit(".", 1)[-1] in allowed, (value.label, engine, stderr)
        assert not any(marker in stderr for marker in ("ERROR: AddressSanitizer", "ERROR: LeakSanitizer", "runtime error:")), stderr
    else:
        assert process.returncode == 0 and not stderr, (value.label, engine, process.returncode, stderr)
    assert actual == expected["files"] and directories == expected["directories"], (value.label, engine, "complete file effects differ")
    return {"command": list(process.args), "returncode": process.returncode, "stdout": stdout, "stderr": stderr,
            "files": {name: {"sha256": digest(data), "bytes": len(data)} for name, data in sorted(actual.items())},
            "directories": directories}


def validate_commands(commands, timeout=90, include_real=True, compute=False):
    results = []
    case_factory, expected_result = (compute_cases, compute_oracle) if compute else (cases, oracle)
    for value, error, original in case_factory(include_real):
        expected = expected_result(value, error, original)
        engines = {}
        excluded = {}
        for name, command in commands.items():
            if name == "original" and not original:
                excluded[name] = "This fixture tests the explicitly stricter typed-schema or architecture contract."
                continue
            codon_exclusion = next((reason for label, reason in CODON_EXCLUSIONS.items()
                                    if value.label == label or value.label.startswith(label + "-compute-")), None)
            if name == "codon" and codon_exclusion is not None:
                excluded[name] = codon_exclusion
                continue
            with tempfile.TemporaryDirectory(prefix="catalog-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                         cwd=directory, env=environment(), timeout=timeout, check=False)
                engines[name] = check_result(value, expected, process, directory, name)
        results.append({"case": case_record(value), "expected_error": error, "upstream_domain": original,
                        "engines": engines, "excluded_engines": excluded})
    return {"mode": "repeated-full-compute" if compute else "complete-process",
            "cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "results": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--codon", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--skip-real", action="store_true")
    parser.add_argument("--compute", action="store_true", help="validate the repeated full-compute entrypoints")
    args = parser.parse_args()
    if sys.version_info < (3, 14):
        raise SystemExit("run this validator with CPython 3.14 or newer")
    commands = {"original": original_command(args.python, args.compute), "port": python_command(args.python, args.compute)}
    for name in ("binary", "rust", "codon"):
        value = getattr(args, name)
        if value:
            commands["imre" if name == "binary" else name] = [str(value.resolve(strict=True))]
            if name == "rust" and args.compute:
                commands[name].append("--compute")
    result = validate_commands(commands, include_real=not args.skip_real, compute=args.compute)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Validated {result['cases']} catalog cases across {result['engine_executions']} engine runs")


if __name__ == "__main__":
    main()
