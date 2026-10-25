"""Check real process arguments in Imre's two launch modes (not upstream IDs)."""

import argparse
import ast
import os
from pathlib import Path
import subprocess
import tempfile


PROGRAM = """\
import sys
from sys import argv, orig_argv

print(argv)
print(orig_argv)
argv.append("script-added")
orig_argv.append("launcher-added")
print(sys.argv[-1])
print(sys.orig_argv[-1])
"""


def output(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=45)
    if result.returncode:
        raise RuntimeError(f"{command[:2]}: {result.stderr}")
    return result.stdout.splitlines()


def check(command, script_argv):
    argv, original, script_last, original_last = output(command)
    assert ast.literal_eval(argv) == script_argv
    assert ast.literal_eval(original) == command
    assert (script_last, original_last) == ("script-added", "launcher-added")


def check_commands(compiler, flags, root):
    """Exercise the option/argument boundary, real startup flags and isolated imports."""
    import sys

    def compare(options, code, arguments=(), changes=None, attached=False):
        env = dict(os.environ, IMRE_ENCODED_FLAGS="\x1f".join([*flags, "--no-cache"]))
        if changes:
            env.update(changes)
        suffix = [*options, *(["-c" + code] if attached else ["-c", code]), *arguments]
        stock = subprocess.run([sys.executable, *suffix], cwd=root, env=env, capture_output=True, timeout=45)
        native = subprocess.run([compiler, *suffix], cwd=root, env=env, capture_output=True, timeout=45)
        assert (native.returncode, native.stdout, native.stderr) == (stock.returncode, stock.stdout, stock.stderr), (suffix, native, stock)
        return native.stdout

    program = "import sys\nprint(sys.argv)\nprint(sys.orig_argv[1:])"
    compare(["-IXutf8"], program, ["", "-X", "two words", "café"], attached=True)
    compare([], program, ["-I", "--", "-c"])
    fields = ("isolated", "ignore_environment", "safe_path", "no_user_site", "dont_write_bytecode", "utf8_mode")
    flag_code = "import sys\n" + "\n".join("print(sys.flags." + name + ")" for name in fields) + "\nprint(sys.dont_write_bytecode)"
    env = {"LC_ALL": "C.UTF-8", "PYTHONUTF8": "1", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1", "PYTHONSAFEPATH": "1"}
    compare([], flag_code, changes=env)
    compare(["-I"], flag_code, changes=env)
    compare(["-BI", "-X", "utf8=0"], flag_code, changes=env)
    compare(["-I", "-Xutf8=1"], flag_code, changes=dict(env, PYTHONUTF8="invalid"))
    compare(["-Xutf8=1", "-X", "utf8=0"], flag_code, changes=env)
    compare(["-Xutf8=0", "-Xutf8=1"], flag_code, changes=env)
    compare([], flag_code, changes=dict(env, PYTHONNOUSERSITE="\u00a00", PYTHONDONTWRITEBYTECODE="\u20030"))
    compare([], flag_code, changes=dict(env, PYTHONNOUSERSITE="\t0", PYTHONDONTWRITEBYTECODE="\v0"))
    compare([], "     print(123)\n", changes=env)  # 3.14 dedents the command
    compare([], "\n\tif 1:\n\t\tprint('tab')\n\tprint('end')\n")
    compare(["-I"], "    import sys\n    print(sys.orig_argv[3])", ["kept"])
    compare([], "# coding: latin-1\nprint('é')")
    compare([], "try:\n    print(__file__)\nexcept NameError as error:\n    print(str(error))")
    assert subprocess.run([compiler, "-I", "-c", "import sys; print(sys.executable)"],
                          capture_output=True, text=True, check=True, timeout=45).stdout.strip() == compiler

    (root / "local_command_helper.py").write_text("def value() -> int:\n    return 19\n", encoding="utf-8")
    local_code = "from local_command_helper import value\nprint(value())"
    compare([], local_code, changes={"PYTHONSAFEPATH": ""})
    for exe in (sys.executable, compiler):
        isolated = subprocess.run([exe, "-I", "-c", local_code], cwd=root, capture_output=True, timeout=45)
        assert isolated.returncode and not isolated.stdout, (exe, isolated)
    path_root = root / "pythonpath"
    path_root.mkdir()
    (path_root / "extra_command_helper.py").write_text("def number() -> int:\n    return 23\n", encoding="utf-8")
    path_code = "from extra_command_helper import number\nprint(number())"
    compare([], path_code, changes={"PYTHONPATH": str(path_root), "PYTHONSAFEPATH": "1"})
    for exe in (sys.executable, compiler):
        isolated = subprocess.run([exe, "-I", "-c", path_code], cwd=root,
                                  env=dict(os.environ, PYTHONPATH=str(path_root)), capture_output=True, timeout=45)
        assert isolated.returncode and not isolated.stdout, (exe, isolated)
        mixed = subprocess.run([exe, "-c", " \tif 1:\n\t \t\t print('bad')"], capture_output=True, timeout=45)
        assert mixed.returncode and not mixed.stdout, (exe, mixed)
    for options in (["-c"], ["-X"], ["-I", "-X", "utf8=invalid", "-c", "print(99)"]):
        invalid = subprocess.run([compiler, *options], cwd=root, capture_output=True, timeout=45)
        assert invalid.returncode and not invalid.stdout, (options, invalid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    compiler = str(args.imre.resolve())
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang,
             "--opt-level", args.opt_level, "--quiet"]
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        check_commands(compiler, flags, root)
        source = str(root / "program.py")
        binary = str(root / "program")
        Path(source).write_text(PROGRAM, encoding="utf-8")
        output([compiler, "build", source, *flags, "-o", binary])
        # On Linux the second vector exceeds the startup socket's buffer.
        # Allow for environments with a smaller exec limit.
        chunks = min(20, os.sysconf("SC_ARG_MAX") // (2 * 32768))
        for values in (["", "café 🦀", "--", "two words"],
                       [str(i) + "a" * 32760 for i in range(chunks)]):
            check([binary, *values], [binary, *values])
            check([compiler, "run", source, *flags, "--", *values], [source, *values])

        # Reading only orig_argv must still request the launcher's vector.
        Path(source).write_text("import sys\nprint(sys.orig_argv)\n", encoding="utf-8")
        command = [compiler, "run", source, *flags, "--", ""]
        assert ast.literal_eval(output(command)[0]) == command
    print(f"sys argv boundaries passed at -O{args.opt_level}")


if __name__ == "__main__":
    main()
