"""Exercise deferred JSON operations and boxed decoding under Linux memory limits."""
import argparse
import os
from pathlib import Path
import resource
import select
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


def check_boxed_input(argv, document, kind):
    with subprocess.Popen([*argv, document, kind], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
        try:
            assert select.select([child.stdout], [], [], 30)[0], "input setup timed out"
            assert child.stdout.readline() == b"ready\n"
            # Limit only decoding. The source and its interpreter are already resident.
            statm = Path(f"/proc/{child.pid}/statm").read_text().split()
            cap = int(statm[0]) * os.sysconf("SC_PAGESIZE") + 512 * 1024
            resource.prlimit(child.pid, resource.RLIMIT_AS, (cap, cap))
            output, errors = child.communicate(b"x", timeout=30)
        finally:
            child.kill()
    assert (child.returncode, errors) == (0, b""), (argv, child.returncode, errors)
    assert output in (b"refused\n0\n", b"0\n0\n"), (argv, output)


def check(args, temp):
    marker = temp / "parsed"
    paths = {}
    for n in (10_000, 650_000):
        path = temp / f"{n}.json"
        path.write_bytes(b'{"x":[' + b'{"v":1},' * (n - 1) + b'{"v":1}]}')
        paths[n] = path
    boxed_source = SOURCE.with_name("boxed_input.py")
    document = temp / "boxed.json"
    document.write_bytes(b" " * (4 * 1024 * 1024) + b"[]")
    binaries = []
    boxed_commands = [[args.python, boxed_source]]
    for level in ("0", "3"):
        exe = temp / f"retry-O{level}"
        subprocess.run([args.compiler.resolve(), "build", SOURCE, "--quiet", "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang,
                        "--opt-level", level, "-o", exe], check=True)
        binaries.append(exe)
        boxed = temp / f"boxed-O{level}"
        subprocess.run([args.compiler.resolve(), "build", boxed_source, "--quiet", "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang,
                        "--opt-level", level, "-o", boxed], check=True)
        boxed_commands.append([boxed])
    for argv in boxed_commands:
        for kind in ("str", "bytes"):
            check_boxed_input(argv, document, kind)
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
    print("18 CPython and native JSON allocation cases passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as directory:
        check(args, Path(directory))
