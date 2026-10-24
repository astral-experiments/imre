"""Check the guarded length rewrite and its missing-guard mutant on bounded heaps."""

import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SOURCE = Path(__file__).parent / "fixtures" / "containers" / "guarded_list_length.py"


def instrument(ir: str, force_distinct: bool) -> str:
    # Only scan() has this loop version. Restrict the mutation to its definition.
    bodies = re.findall(r"^define [^\n]+\{\n.*?^\}", ir, re.MULTILINE | re.DOTALL)
    (body,) = [body for body in bodies if "call i32 @relar_list_all_i32(" in body]
    guard = r"icmp ne ptr %v\d+, %v\d+"
    (comparison,) = re.findall(guard, body)
    fast = "  call void @relar_list_reserve_range("
    assert body.count(fast) == 1
    # One byte on stderr per fast-arm entry, without affecting the program output.
    traced = body.replace(
        fast, "  call i64 @write(i32 2, ptr @fast_marker, i64 1)\n" + fast
    )
    if force_distinct:
        traced = traced.replace(comparison, "icmp eq i1 true, true")
    return ir.replace(body, traced) + (
        "\n@fast_marker = private constant i8 70\ndeclare i64 @write(i32, ptr, i64)\n"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    reference = subprocess.check_output(
        [sys.executable, SOURCE], timeout=30
    ).splitlines()
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        llvm = directory / "scan.ll"
        binary = directory / "scan"
        build = [
            args.compiler.resolve(),
            "build",
            SOURCE,
            "--runtime",
            args.runtime,
            "--clang",
            args.clang,
            "--emit-llvm",
            llvm,
            "-o",
            binary,
            "-q",
        ]
        if args.sanitize:
            build.append("--sanitize")
        subprocess.run(build, check=True)
        assert subprocess.check_output([binary], timeout=30).splitlines() == reference
        original = llvm.read_text()
        libraries = (
            ["-liconv"]
            if sys.platform == "darwin"
            else ["-ldl", "-lpthread", "-lm", "-lrt", "-lutil"]
        )
        if args.sanitize:
            libraries.append("-fsanitize=address")
        for force_distinct in (False, True):
            llvm.write_text(instrument(original, force_distinct))
            subprocess.run(
                [
                    args.clang,
                    "-O0",
                    "-Wno-override-module",
                    llvm,
                    args.runtime,
                    *libraries,
                    "-o",
                    binary,
                ],
                check=True,
            )
            result = subprocess.run(
                [binary], check=True, capture_output=True, timeout=30
            )
            # Six nonempty inputs, each paired with seven distinct output lists.
            # Forcing the guard admits the six nonempty aliased pairs as well.
            assert result.stderr == b"F" * (48 if force_distinct else 42), result.stderr
            lines = result.stdout.splitlines()
            assert len(lines) == len(reference) == 56 * 3
            mismatches = []
            for k in range(0, len(lines), 3):
                assert lines[k + 1 : k + 3] == reference[k + 1 : k + 3], (
                    lines[k],
                    "list contents changed",
                )
                if lines[k] != reference[k]:
                    mismatches.append((reference[k].split(), lines[k].split()))
            if force_distinct:
                assert [want[:2] for want, _ in mismatches] == [
                    [str(i).encode(), b"-1"] for i in range(1, 7)
                ], mismatches
                want, got = mismatches[0]
                assert want[2:] == [b"2", b"2"] and got[2:] == [b"1", b"0"], mismatches[
                    0
                ]
            else:
                assert not mismatches, mismatches
    print("56 guarded heaps matched CPython (42 fast-arm entries).")
    print(
        "Forcing distinctness failed on 6 aliased heaps. For xs = ys = [0] and append(1), "
        "CPython visited 2 elements (sum 2) and the mutant visited 1 (sum 0)."
    )


if __name__ == "__main__":
    main()
