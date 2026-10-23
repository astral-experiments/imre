"""Generate the Relar and CPython scenic-score replays from the pinned upstream files.

Use tree_house_grid.py and input/sample_input.txt from
derailed-dash/Advent-of-Code at 1e795c921cd9f824dba16df59eda61d9df33e8d7,
under src/AoC_2022/d08_treetop_tree_house.
"""

import argparse
import hashlib
import json
from pathlib import Path


SOURCE_SHA256 = "3dac7ca6e13dd7af8ec2736282c244da33ab4b77c4737ddf9b2e2413ee11fde2"
SAMPLE_SHA256 = "3ddb8247902880cce5a926151eab994ac5274300114e08ea0d0704fba97f0c64"

GRID_INIT = """\
class Grid:
    rows: list[list[int]]
    _width: int
    _height: int

    def __init__(self, rows: list[list[int]]) -> None:
        self.rows = rows
        self._width = len(rows[0])
        self._height = len(rows)

"""

# Relar uses explicit loops for the upstream scorer's four stored generators.
SCORE_POINT = """\
    def get_scenic_score_for_point(self, point: Point) -> int:
        x = point.x
        y = point.y
        h = self.rows[y][x]
        result = 1
        for dx, dy in [(-1,0),(1,0),(0,-1),(0,1)]:
            i = x + dx
            j = y + dy
            count = 0
            while 0 <= i < self._width and 0 <= j < self._height:
                count += 1
                if self.rows[j][i] >= h:
                    break
                i += dx
                j += dy
            result *= count
        return result

"""

PRINT_SCORES = """\
for score in Grid(trees).get_scenic_scores():
    print(score)
"""

TIMING_DRIVER = """
def repeat(grid: Grid, count: int) -> int:
    checksum = 0
    for i in range(count):
        scores = grid.get_scenic_scores()
        checksum += sum(scores)
    return checksum

print(repeat(Grid(trees), 1500))
"""


def read_pinned(path: Path, expected: str) -> str:
    data = path.read_bytes()
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise SystemExit(f"{path}: expected SHA-256 {expected}, got {actual}")
    return data.decode("utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="upstream tree_house_grid.py")
    parser.add_argument("sample", type=Path, help="upstream input/sample_input.txt")
    parser.add_argument("output", type=Path, help="directory for the three generated programs")
    args = parser.parse_args()

    lines = read_pinned(args.source, SOURCE_SHA256).splitlines(keepends=True)
    sample = read_pinned(args.sample, SAMPLE_SHA256)
    trees = [[int(height) for height in row] for row in sample.splitlines()]
    fixture = f"trees = {json.dumps(trees, separators=(',', ':'))}\n"

    # Copy Point (52–57), Grid.get_scenic_scores (99–111), and the CPython Grid
    # through its scorer (58–139), before the optional visualization methods.
    point = "".join(lines[51:57])
    traversal = "".join(lines[98:111])
    reference_grid = "".join(lines[57:139])
    relar = "from dataclasses import dataclass\n\n" + point + GRID_INIT + traversal + SCORE_POINT
    reference = "from dataclasses import dataclass\nimport math\n\n" + point + reference_grid
    args.output.mkdir(parents=True, exist_ok=True)
    for name, program in [
        ("scenic.py", relar + fixture + PRINT_SCORES),
        ("scenic_reference.py", reference + fixture + PRINT_SCORES),
        ("scenic_timing.py", relar + fixture + TIMING_DRIVER),
    ]:
        (args.output / name).write_text(program, encoding="utf-8")


if __name__ == "__main__":
    main()
