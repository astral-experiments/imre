"""Compare the native slice-sum variants emitted by the compiler test with CPython."""

import os
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

TESTS = Path(__file__).resolve().parent


class Case(NamedTuple):
    heap: int
    origin: int
    shape: int
    effect: int
    stride: int


class Observation(NamedTuple):
    result_or_exception: str
    xs: str
    ys: str
    cleanup: str


def observe(command):
    lines = subprocess.check_output(command, text=True, timeout=60).splitlines()
    observations = {}
    for offset in range(0, len(lines), 5):
        header, outcome, xs, ys, trace = lines[offset : offset + 5]
        marker, *fields = header.split()
        case = Case(*map(int, fields))
        assert marker == "C" and case not in observations, header
        assert outcome.startswith(("R ", "V ", "T ")), (case, outcome)
        assert (xs.split()[0], ys.split()[0], trace.split()[0]) == ("X", "Y", "E"), case
        observations[case] = Observation(outcome, xs, ys, trace)
    assert len(observations) == 1512, f"{command} emitted {len(observations)} cases"
    return observations


def validate(mode, reference, actual):
    assert reference.keys() == actual.keys(), (mode, reference.keys() ^ actual.keys())
    mismatches = [
        case for case, expected in reference.items() if actual[case] != expected
    ]
    cleanup_only = sum(reference[c][:3] == actual[c][:3] for c in mismatches)
    print(
        f"{mode}: {len(mismatches)}/{len(reference)} differ ({cleanup_only} cleanup only)"
    )
    for case in mismatches:
        # The live-list control and the inputs outside each mutation must still agree.
        if (mode == "erase-full-slice" and (case.origin, case.shape) == (0, 1)) or (
            mode == "start-first" and case.shape in (1, 2)
        ):
            continue
        raise AssertionError(
            f"{mode} {case}\nCPython {reference[case]}\nRelar   {actual[case]}"
        )

    def witness(case, expected, mutated):
        observed = reference[case], actual[case]
        assert observed == (expected, mutated), f"{mode} {case}: {observed}"

    if mode == "erase-full-slice":
        # xs = ys = [1]. start appends 2; both references then see [1, 2].
        snapshot = Observation("R 1", "X 1 2", "Y 1 2", "E 20 21 30")
        live = snapshot._replace(result_or_exception="R 3")
        witness(Case(1, 0, 1, 1, 0), snapshot, live)
        assert reference[Case(1, 0, 0, 1, 0)] == live
    elif mode == "start-first":
        # A zero stride fails before the start callback can append and raise.
        witness(
            Case(1, 0, 2, 2, 2),
            Observation("V slice step cannot be zero", "X 1", "Y 1", "E 10 11 30"),
            Observation("V start", "X 1 2", "Y 1 2", "E 20 21 30"),
        )
        witness(
            Case(1, 0, 2, 2, 3),
            Observation("T bound", "X 1", "Y 1", "E 10 11 30"),
            Observation("V start", "X 1 2", "Y 1 2", "E 20 21 30"),
        )
        # Both calls return 1, but their finally blocks close the files in opposite orders.
        closed = Observation("R 1", "X 1", "Y 1", "E 10 11 20 21 30")
        witness(
            Case(1, 0, 2, 0, 0), closed, closed._replace(cleanup="E 20 21 10 11 30")
        )


def main():
    runtime = Path(
        os.environ.get(
            "RELAR_RUNTIME", TESTS.parent / "target/debug/librelar_runtime.a"
        )
    )
    assert runtime.is_file(), (
        f"Build relar-runtime or set RELAR_RUNTIME to its absolute path: {runtime}"
    )
    reference = observe([sys.executable, TESTS / "fixtures/bounded_slice_sum.py"])
    libraries = (
        ["-liconv"]
        if sys.platform == "darwin"
        else ["-ldl", "-lpthread", "-lm", "-lrt", "-lutil"]
    )
    for mode in ("materialized", "fused", "erase-full-slice", "start-first"):
        subprocess.run(
            [
                os.environ.get("CLANG", "clang"),
                "-O3",
                "-Wno-override-module",
                f"{mode}.ll",
                runtime,
                *libraries,
                "-o",
                mode,
            ],
            check=True,
            timeout=60,
        )
        validate(mode, reference, observe([f"./{mode}"]))


if __name__ == "__main__":
    main()
