"""Compare native whole-file unittest discovery, fixture effects and outcomes with CPython."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

FIXTURES = Path(__file__).resolve().parent / "fixtures/unittest_main"
DURATION = re.compile(r"(Ran \d+ tests? in )\d+\.\d+s")


def invoke(command):
    return subprocess.run(command, text=True, capture_output=True, timeout=180)


def report(stderr, failed_fixture):
    output = DURATION.sub(r"\1<duration>s", stderr)
    if not failed_fixture:
        return output
    # Native caught exceptions do not yet carry CPython traceback frames.
    # Compare order and identity of every reported result, status and totals.
    return [
        line
        for line in output.splitlines()
        if line.startswith(
            ("test_", "FAIL:", "ERROR:", "Ran ", "OK", "FAILED", "NO TESTS RAN")
        )
    ]


def compare(a, source, work, *, upstream=False):
    flags = [["-v"]] if upstream else [[], ["-v"], ["-q"], ["-f"]]
    refs = [(opt, invoke([a.python, "-E", "-B", source, *opt])) for opt in flags]
    failed = not upstream and source.stem in {"lifecycle", "setup_fields", "identity"}
    if upstream and any(
        ref.returncode or not re.search(r"^Ran [1-9]\d* tests? in ", ref.stderr, re.M)
        for _, ref in refs
    ):
        raise ValueError(
            "the pinned upstream reference did not successfully execute any tests"
        )
    if not upstream:
        expected = 1 if failed else (5 if source.stem == "empty" else 0)
        if any(ref.returncode != expected for _, ref in refs):
            raise AssertionError((source, refs))
    checks = 0
    for level in a.opt_levels:
        for reachable in [False, True]:
            binary = work / "suite"
            cmd = [
                a.compiler,
                "build",
                source,
                "-o",
                binary,
                "--no-cache",
                "--opt-level",
                str(level),
                "--runtime",
                a.runtime,
                "--clang",
                a.clang,
            ]
            if reachable:
                cmd.append("--reachable-only")
            if a.sanitize:
                cmd.append("--sanitize")
            built = invoke(cmd)
            if built.returncode:
                raise RuntimeError(built.stderr)
            for opt, reference in refs:
                native = invoke([binary, *opt])
                if re.search(
                    r"(?:ERROR|SUMMARY): (?:Address|Leak)Sanitizer:", native.stderr
                ):
                    raise AssertionError(native.stderr)

                def observation(x):
                    return x.returncode, x.stdout, report(x.stderr, failed)

                if observation(reference) != observation(native):
                    raise AssertionError(
                        (
                            source,
                            level,
                            reachable,
                            opt,
                            observation(reference),
                            observation(native),
                        )
                    )
                checks += 1
    return checks


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--clang", default="clang")
    p.add_argument("--python", default=sys.executable)
    p.add_argument("--sanitize", action="store_true")
    p.add_argument(
        "--opt-levels", nargs="+", type=int, choices=range(4), default=[0, 3]
    )
    p.add_argument(
        "--upstream-source",
        type=Path,
        help="run an intact standalone CPython suite instead of the controls",
    )
    p.add_argument(
        "--manifest", type=Path, help="UPSTREAM.json for the supplied CPython file"
    )
    a = p.parse_args()
    a.compiler, a.runtime = a.compiler.resolve(), a.runtime.resolve()
    if bool(a.upstream_source) != bool(a.manifest):
        p.error("supply the upstream source and its manifest together")
    if a.upstream_source:
        source = a.upstream_source.resolve()
        metadata = json.loads(a.manifest.read_bytes())
        path = source.relative_to(a.manifest.resolve().parent / "upstream").as_posix()
        data = source.read_bytes()
        blob = hashlib.sha1(
            b"blob " + str(len(data)).encode() + b"\x00" + data
        ).hexdigest()
        if metadata["files"].get(path) != blob:
            raise ValueError(
                "the standalone test differs from its pinned upstream blob"
            )
        version = subprocess.check_output(
            [a.python, "-c", "import platform; print(platform.python_version())"],
            text=True,
        ).strip()
        if metadata["tag"] != "v" + version:
            raise ValueError(
                "the reference Python version differs from the pinned test"
            )
        sources = [source]
    else:
        sources = [
            FIXTURES / name for name in ("discover.py", "lifecycle.py", "setup_fields.py", "identity.py", "empty.py")
        ]
    with tempfile.TemporaryDirectory() as td:
        checks = sum(
            compare(a, source, Path(td), upstream=a.upstream_source is not None)
            for source in sources
        )
        if not a.upstream_source:
            # A custom loader or lifecycle override must never turn into a
            # passing run that silently omitted its behavior.
            for definition, message in (
                ("def load_tests():\n    pass", "module hooks"),
                (
                    "class Custom(TestCase):\n    def run(self) -> None:\n        pass",
                    "custom result hooks",
                ),
                (
                    "class Custom(TestCase):\n    def __init__(self) -> None:\n        super().__init__()",
                    "inherited TestCase initializer",
                ),
            ):
                if definition.startswith("class "):
                    definition += "\n    def test_body(self) -> None:\n        pass"
                case = Path(td) / "custom.py"
                case.write_text(
                    "from unittest import TestCase, main\n" + definition + "\nmain()\n"
                )
                for mode in ([], ["--reachable-only"]):
                    rejected = invoke([a.compiler, "check", case, *mode])
                    if not rejected.returncode or message not in rejected.stderr:
                        raise AssertionError((definition, mode, rejected.stderr))
    print(f"Passed {checks} whole-file unittest comparisons")


if __name__ == "__main__":
    main()
