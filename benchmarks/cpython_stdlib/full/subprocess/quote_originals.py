"""Observe each command-line result and its assertion in both original test classes."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

HERE = Path(__file__).resolve().parent
IDS = tuple(
    "test.test_subprocess." + cls + ".test_list2cmdline"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def original_arguments(body):
    """Count the actual path arguments that feed each upstream equality comparison."""
    [cls] = ast.parse(b"class Original:\n" + body).body
    [method] = cls.body
    arities = []
    for statement in method.body:
        comparison = statement.value
        assert comparison.func.attr == "assertEqual"
        call, expected = comparison.args
        assert (
            call.func.value.id == "subprocess"
            and call.func.attr == "list2cmdline"
            and isinstance(expected.value, str)
        )
        [words] = call.args
        arities.append(len(words.elts))
    assert len(arities) == 8
    return arities


def measure(tools, case, original, wrapper):
    name = case.id()
    with tempfile.TemporaryDirectory(prefix="imre-subprocess-quote-") as tmp:
        work = Path(tmp)
        owner, preamble = fixture(case, work)
        body = method_bytes(SOURCE, owner, case._testMethodName)
        arities = original_arguments(body)
        execute = native_method(tools, work, SOURCE, owner, name, preamble)
        untouched = execute(case, original)
        events = work / "events"
        binary = work / "observed"
        subprocess.run(
            [
                tools.imre,
                "build",
                work / "case.py",
                "--no-cache",
                "--opt-level",
                str(tools.level),
                "--runtime",
                tools.runtime,
                "--clang",
                tools.clang,
                "-C",
                f"link-arg={wrapper}",
                "-C",
                "link-arg=-Wl,--wrap=imre_subprocess_quote_argument",
                "-C",
                "link-arg=-Wl,--wrap=imre_str_join",
                "-o",
                binary,
            ],
            check=True,
            capture_output=True,
            timeout=180,
        )
        env = dict(os.environ, IMRE_QUOTE_EVENTS=str(events))
        env.pop("IMRE_QUOTE_CORRUPT", None)
        expected = ["Q" * n + "J" for n in arities]
        for index in range(len(expected) + 1):
            events.write_bytes(b"")
            if index:
                env["IMRE_QUOTE_CORRUPT"] = str(index)
            result = subprocess.run([binary], capture_output=True, env=env, timeout=30)
            reached = events.read_text()
            if index:
                assert result.returncode and b"AssertionError" in result.stderr, result
                assert b"control: wrong command line" in result.stderr, result.stderr
                assert not result.stdout and reached == "".join(expected[:index]), (
                    index,
                    reached,
                )
            else:
                assert result.returncode == 0 and not result.stderr, result
                assert result.stdout == f"completed {name}\n".encode()
                assert reached == "".join(expected), reached
        return {
            **untouched,
            "native_program_sha256": digest(work / "case"),
            "observed_program_sha256": digest(binary),
            "arguments_per_original_call": arities,
            "assertions_rejecting_substituted_result": list(range(1, len(arities) + 1)),
            "native_argument_quotes": sum(arities),
            "native_joins": len(arities),
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tools = Toolchain(
        *(p.resolve() for p in (args.imre, args.runtime, args.clang)), args.opt_level
    )
    with tempfile.TemporaryDirectory(prefix="imre-subprocess-quote-link-") as tmp:
        wrapper = Path(tmp) / "observe.o"
        subprocess.run(
            [
                tools.clang,
                "-Wall",
                "-Wextra",
                "-Werror",
                "-c",
                HERE / "fixture_quote.c",
                "-o",
                wrapper,
            ],
            check=True,
        )

        def execute(case, original):
            return measure(tools, case, original, wrapper)

        evidence, errors, skips = run_in_fixtures(
            load_suite(manifest["module"]), dict.fromkeys(IDS, execute)
        )
    if errors or skips or set(evidence) != set(IDS):
        raise AssertionError((errors, skips, set(evidence)))
    report = dict(
        cpython_commit=manifest["commit"],
        opt_level=tools.level,
        toolchain={
            key: digest(getattr(tools, key)) for key in ("imre", "runtime", "clang")
        },
        observer_sha256=digest(HERE / "fixture_quote.c"),
        rows=evidence,
    )
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
