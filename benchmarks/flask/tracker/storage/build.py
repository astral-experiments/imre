"""Build the bytes-only Imre extension against the host SQLite library."""

import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent


def verify(compiler: Path, runtime: Path, clang: str, extension: Path, work: Path) -> str:
    """Write each database once, then read both with each implementation."""
    work.mkdir(parents=True)
    native = work / "native"
    stock = work / "cpython"
    for graph, adapter in ((native, HERE), (stock, HERE / "cpython")):
        graph.mkdir()
        shutil.copyfile(HERE / "contract.py", graph / "contract.py")
        shutil.copyfile(adapter / "tracker_db.py", graph / "tracker_db.py")
    binary = work / "contract"
    subprocess.run(
        [
            compiler,
            "build",
            native / "contract.py",
            "--module-root",
            native,
            "--extension-path",
            extension,
            "--runtime",
            runtime,
            "--clang",
            clang,
            "--opt-level",
            "3",
            "--no-cache",
            "-o",
            binary,
        ],
        check=True,
    )
    backends = [
        ([sys.executable, "-BES", stock / "contract.py"], stock / "data.sqlite"),
        ([binary], native / "data.sqlite"),
    ]
    argument = "caller input"
    for command, database in backends:
        subprocess.run([*command, "write", database, argument], check=True)
    outputs = {
        subprocess.check_output([*command, "read", database, argument])
        for command, _ in backends
        for _, database in backends
    }
    if len(outputs) != 1:
        raise RuntimeError(f"SQLite contract produced different rows after restart: {outputs!r}")
    output = outputs.pop()
    (work / "rows.txt").write_bytes(output)
    # Hash the observed values, independent of SQLite file header/library version.
    return hashlib.sha256(output).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--cc", default="cc")
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        scratch = Path(tmp)
        header = subprocess.check_output([args.imre, "extension-header"])
        (scratch / "Python.h").write_bytes(header)
        obj = scratch / "sqlite.o"
        subprocess.run(
            [
                args.cc,
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fPIC",
                "-I",
                scratch,
                "-c",
                HERE / "sqlite.c",
                "-o",
                obj,
            ],
            check=True,
        )
        subprocess.run(["ar", "rcs", args.output_dir / "_tracker_sqlite.imre.a", obj], check=True)
    shutil.copyfile(HERE / "_tracker_sqlite.pyi", args.output_dir / "_tracker_sqlite.pyi")
    (args.output_dir / "_tracker_sqlite.imre.libs").write_text("sqlite3\n")


if __name__ == "__main__":
    main()
