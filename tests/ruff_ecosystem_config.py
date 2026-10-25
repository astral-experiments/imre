"""Differentially exercise Ruff's original ecosystem config discovery and patching."""

import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/ruff_ecosystem/config_patch.py"
NAMES = (".ruff.toml", "ruff.toml", "pyproject.toml")
CONFIG = (
    '# original bytes ☃\nrequired-version="0.1"\n'
    '[lint]\nselect = ["unused-import", "missing-code", "F401"]\n'
    '[lint.per-file-ignores]\n"test.py" = ["unused-import"]\n'
)
PYPROJECT = (
    '# use the tool.ruff table\n[project]\nname = "unchanged"\n'
    'released = 1979-05-27T07:32:00.101Z\n'
    'small = 0xdead_beef\nscale = 1e3\n'
    + CONFIG.replace("required-version=", "[tool.ruff]\nrequired-version=")
    .replace("[lint]", "[tool.ruff.lint]")
    .replace("[lint.per-file-ignores]", "[tool.ruff.lint.per-file-ignores]")
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--ruff", type=Path, help="also run discovery using this Ruff executable")
    args = parser.parse_args()
    commands = ([args.python, "-B", str(SOURCE)], [str(args.binary.resolve())])

    env = {**os.environ, "PYTHONPATH": str(SOURCE.parent)}
    outputs = [subprocess.check_output([*command, "defaults"], env=env, timeout=30) for command in commands]
    assert outputs[0].startswith(b"58\n") and outputs[0] == outputs[1]

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        wrapper = root / "ruff"
        wrapper.write_text(
            "#!/bin/sh\nprintf x >> \"$RUFF_CALLS\"\n"
            'test "$*" = "rule --all --output-format json" || exit 1\n'
            'if [ -n "$REAL_RUFF" ]; then exec "$REAL_RUFF" "$@"; fi\n'
            'printf \'%s\\n\' \'[{"name":"unused-import","code":"F401"},'
            '{"name":"missing-code","code":null}]\'\n',
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        workspace = root / "checkout"
        workspace.mkdir()
        cases = (*[(n, False) for n in range(4)], (0, True))
        for backend in ["", *([str(args.ruff.resolve())] if args.ruff else [])]:
            for count, empty in cases:
                expected_files = {
                    name: (PYPROJECT if name == NAMES[-1] else CONFIG).encode()
                    for name in NAMES[len(NAMES) - count :]
                }
                outputs = []
                for command in commands:
                    for name in NAMES:
                        (workspace / name).unlink(missing_ok=True)
                    for name, content in expected_files.items():
                        (workspace / name).write_bytes(content)
                    calls = root / "calls"
                    calls.write_bytes(b"")
                    p = subprocess.run(
                        [*command, str(workspace), str(wrapper), "empty" if empty else "overrides"],
                        env={**env, "RUFF_CALLS": str(calls), "REAL_RUFF": backend},
                        capture_output=True, timeout=30,
                    )
                    assert p.returncode == 0, (command, p.stderr)
                    assert calls.read_bytes() == b"x", (command, "rule query count")
                    assert {p.name: p.read_bytes() for p in workspace.iterdir()} == expected_files
                    outputs.append(p.stdout)
                assert outputs[0] == outputs[1], (count, empty, outputs)
                assert b"T999\n" in outputs[0] and b"failed inside patch\n" in outputs[0]
    print("All 58 default targets and preview copies matched, along with config selection, normalization, caching, and restoration.")


if __name__ == "__main__":
    main()
