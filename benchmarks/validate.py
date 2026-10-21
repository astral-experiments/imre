"""Validate the route planner against CPython and a small-grid Bellman-Ford oracle."""

import argparse
import json
from pathlib import Path
import subprocess
import sys


SOURCE = Path(__file__).resolve().parents[1] / "examples" / "route_planner.py"


def input_text(width: int, routes: int, seed: int) -> str:
    return f"{width}\n{routes}\n{seed}\n"


def execute(command: list[str], stdin: str) -> str:
    result = subprocess.run(command, input=stdin, text=True, capture_output=True, timeout=120)
    if result.returncode or result.stderr:
        raise RuntimeError(f"{command[0]} failed ({result.returncode}): {result.stderr}")
    return result.stdout


def reference_cost(width: int, seed: int) -> int:
    weights = []
    for _ in range(width * width):
        seed = (seed * 1664525 + 1013904223) % 2**32
        weights.append(seed % 9 + 1)
    distances = [float("inf")] * len(weights)
    distances[0] = 0
    # Repeated relaxation deliberately avoids the example's heap and traversal.
    for _ in range(len(weights) - 1):
        changed = False
        for row in range(width):
            for column in range(width):
                node = row * width + column
                for dr, dc in ((-1, 0), (1, 0), (0, -1), (0, 1)):
                    nr, nc = row + dr, column + dc
                    if 0 <= nr < width and 0 <= nc < width:
                        neighbor = nr * width + nc
                        candidate = distances[node] + weights[neighbor]
                        if candidate < distances[neighbor]:
                            distances[neighbor] = candidate
                            changed = True
        if not changed:
            break
    return int(distances[-1])


def validate(binary: Path, python: str, source: Path = SOURCE) -> dict[str, int | str]:
    cases = [(width, 3, seed) for width in (1, 2, 3, 7, 12) for seed in (0, 1, 42, -17)]
    cases.extend([(7, 2, 2**127), (7, 2, -(2**127))])
    routes_checked = 0
    for width, routes, seed in cases:
        stdin = input_text(width, routes, seed)
        reference = execute([python, "-I", str(source)], stdin)
        native = execute([str(binary)], stdin)
        if native != reference:
            raise AssertionError((width, routes, seed, native, reference))
        values = [int(line) for line in native.splitlines()]
        assert len(values) == routes * 3, (width, routes, seed, values)
        for route in range(routes):
            cost, steps, settled = values[route * 3 : route * 3 + 3]
            expected = reference_cost(width, seed + route)
            assert cost == expected, (width, seed + route, cost, expected)
            assert 2 * (width - 1) <= steps < width * width
            assert 1 <= settled <= width * width
            routes_checked += 1
    return {"input_cases": len(cases), "routes": routes_checked, "oracle": "Bellman-Ford repeated relaxation"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    print(json.dumps(validate(args.binary.resolve(), args.python), indent=2))


if __name__ == "__main__":
    main()
