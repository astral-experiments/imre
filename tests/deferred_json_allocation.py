"""Exercise deferred JSON equality and repr under a real Linux memory limit."""
import argparse
from pathlib import Path
import resource
import subprocess
import sys
import tempfile

SOURCE = Path(__file__).resolve().parent / "fixtures/deferred_json_allocation/retry.py"


def limited():
    cap = 128 * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (cap, cap))


def run(argv, path, marker, mode):
    marker.unlink(missing_ok=True)
    proc = subprocess.run([*argv, path, marker, mode], preexec_fn=limited,
                          capture_output=True, text=True, timeout=90)
    assert (proc.returncode, proc.stderr) == (0, ""), (argv, proc)
    return proc.stdout.splitlines(), marker.is_file()


def check(args, temp):
    marker = temp / "parsed"
    paths = {}
    for n in (10_000, 650_000):
        path = temp / f"{n}.json"
        path.write_bytes(b'{"x":[' + b'{"v":1},' * (n - 1) + b'{"v":1}]}')
        paths[n] = path
    binaries = []
    for level in ("0", "3"):
        exe = temp / f"retry-O{level}"
        subprocess.run([args.compiler.resolve(), "build", SOURCE, "--quiet", "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang,
                        "--opt-level", level, "-o", exe], check=True)
        binaries.append(exe)
    for mode in ("equality", "repr"):
        normal, parsed = run([args.python, SOURCE], paths[10_000], marker, mode)
        assert parsed
        for exe in binaries:
            assert run([exe], paths[10_000], marker, mode) == (normal, True), (mode, exe)
            # Native retries expand the same lazy array; refusal leaves its length usable.
            expected = ["refused 0", "refused 1", "length 650000",
                        f"source {paths[650_000].stat().st_size}"]
            assert run([exe], paths[650_000], marker, mode) == (expected, True), (mode, exe)
        # CPython allocates the rows eagerly and may not reach the marker.
        lines, parsed = run([args.python, SOURCE], paths[650_000], marker, mode)
        assert lines[-1] == f"source {paths[650_000].stat().st_size}"
        assert parsed or lines[0] == "parse refused"
    print("12 CPython and native deferred JSON cases passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as directory:
        check(args, Path(directory))
