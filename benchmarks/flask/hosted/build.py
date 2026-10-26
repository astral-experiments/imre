"""Build a CPython-hosted extension from the checked-in app's Imre LLVM output."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import sysconfig
import tempfile

HERE = Path(__file__).resolve().parent


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--objcopy", default="llvm-objcopy")
    parser.add_argument("--include", type=Path, default=Path(sysconfig.get_path("include")))
    args = parser.parse_args()
    expected = json.loads((HERE.parent / "expected.json").read_text())
    if sys.implementation.name != "cpython" or sys.version.split()[0] != expected["python"]:
        parser.error(f"build using CPython {expected['python']}")
    if sys.platform != "linux" or not (args.include / "Python.h").is_file():
        parser.error("this experiment needs Linux and CPython development headers")

    out = args.out.resolve()
    code = out / "source"
    code.mkdir(parents=True, exist_ok=True)
    app = HERE.parent / "tiny_flask_app.py"
    if digest(app) != expected["app_sha256"]:
        parser.error("app differs from the CPython TCP oracle")
    shutil.copyfile(app, code / app.name)
    # A sys.path observation selects Imre's existing boxed LLVM lowering.
    entry = code / "enable_native.py"
    entry.write_text("import sys\nsys.path\nimport tiny_flask_app\n")
    compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
    llvm = out / "app.ll"
    extension = out / "_imre_views.so"
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        subprocess.run([compiler, "build", entry, "--module-root", code, "--runtime", runtime,
                        "--emit-llvm", llvm, "--opt-level", "0", "--no-cache", "--whole-program",
                        "--clang", args.clang, "-o", work / "standalone"], check=True)
        subprocess.run([args.clang, "-O3", "-fPIC", "-c", llvm, "-o", work / "app.o"], check=True)
        # Use the emitted module-registration routine in the extension.
        subprocess.run([args.objcopy, "--redefine-sym", "main=imre_register_program",
                        work / "app.o", work / "app-module.o"], check=True)
        subprocess.run([args.clang, "-Wall", "-Werror", "-O3", "-shared", "-fPIC",
                        "-I", args.include, HERE / "host.c", work / "app-module.o",
                        "-o", extension], check=True)
    shutil.copyfile(HERE / "launch.py", out / "launch.py")
    manifest = {
        "app_sha256": digest(app),
        "compiler_sha256": digest(compiler),
        "runtime_sha256": digest(runtime),
        "llvm_sha256": digest(llvm),
        "extension_sha256": digest(extension),
        "host_sha256": digest(HERE / "host.c"),
        "launcher_sha256": digest(out / "launch.py"),
        "python": sys.version.split()[0],
    }
    (out / "build.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(out / "build.json")


if __name__ == "__main__":
    main()
