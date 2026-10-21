"""Check ThinLTO linker invocation and protection of linker inputs."""

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SOURCE = Path(__file__).resolve().parent / "programs/short_strings.py"


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command, cwd=cwd, input=b"", capture_output=True, check=False, timeout=120
    )


def require_success(result: subprocess.CompletedProcess[bytes]) -> None:
    assert result.returncode == 0 and not result.stderr, (
        result.args,
        result.stdout,
        result.stderr,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument(
        "--runtime", type=Path, required=True, help="bitcode runtime archive"
    )
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target", required=True)
    parser.add_argument(
        "--rust-lld",
        type=Path,
        required=True,
        help="actual Rust rust-lld backend, not a gcc-ld wrapper",
    )
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    compiler = str(args.compiler.resolve(strict=True))
    runtime = str(args.runtime.resolve(strict=True))
    backend = args.rust_lld.resolve(strict=True)
    clang = str(Path(args.clang).absolute()) if "/" in args.clang else args.clang
    python = str(Path(args.python).absolute()) if "/" in args.python else args.python
    flags = ["--runtime", runtime, "--target", args.target, "--clang", clang]

    with tempfile.TemporaryDirectory(prefix="relar-lto-") as directory:
        temporary = Path(directory)
        linker = temporary / "linker with spaces" / "ld.lld"
        linker.parent.mkdir()
        linker.symlink_to(backend)
        binary = temporary / "short-strings"
        llvm = temporary / "short-strings.ll"
        reference = run([python, "-I", str(SOURCE)], temporary)
        require_success(reference)
        built = run(
            [
                compiler,
                "build",
                str(SOURCE),
                "-o",
                str(binary),
                "--emit-llvm",
                str(llvm),
                "--opt-level",
                "3",
                *flags,
                "--lto-linker",
                str(linker),
            ],
            temporary,
        )
        require_success(built)
        actual = run([str(binary)], temporary)
        require_success(actual)
        assert actual.stdout == reference.stdout, (actual.stdout, reference.stdout)
        assert llvm.stat().st_size > 0

        # Every rejected command uses a disposable linker, never the real backend.
        source = temporary / "guard.py"
        source.write_text("print(1)\n")
        fake = temporary / "fake-linker"
        fake.write_bytes(b"protected scratch linker\n")
        symlink = temporary / "fake-symlink"
        symlink.symlink_to(fake)
        hardlink = temporary / "fake-hardlink"
        os.link(fake, hardlink)
        protected = {
            path: path.read_bytes() for path in (source, fake, symlink, hardlink)
        }
        required = "--lto-linker requires explicit --runtime and --target"
        rejected = [
            (["--lto-linker", str(fake), "--target", args.target], required),
            (["--lto-linker", str(fake), "--runtime", runtime], required),
            (
                [*flags, "--lto-linker", str(temporary / "missing-linker")],
                "ThinLTO linker is not an existing file",
            ),
        ]
        for alias in (fake, symlink, hardlink):
            for output_flag in ("-o", "--emit-llvm"):
                rejected.append(
                    (
                        [*flags, "--lto-linker", str(fake), output_flag, str(alias)],
                        "distinct paths",
                    )
                )
        for options, diagnostic in rejected:
            result = run(
                [
                    compiler,
                    "build",
                    str(source),
                    "-o",
                    str(temporary / "rejected"),
                    *options,
                ],
                temporary,
            )
            for path, contents in protected.items():
                assert path.read_bytes() == contents, f"protected input changed: {path}"
            assert result.returncode != 0 and diagnostic.encode() in result.stderr, (
                result.args,
                result.stdout,
                result.stderr,
            )
    print(f"Passed {1 + len(rejected)} ThinLTO CLI checks")


if __name__ == "__main__":
    main()
