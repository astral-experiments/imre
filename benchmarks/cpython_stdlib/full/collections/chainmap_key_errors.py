"""Refuse a nested tuple's hashing allocation inside one ChainMap operation (Linux)."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


SETUP = """from collections import ChainMap
key = ((1, 2), (3, 4))
old = ((9, 8), (7, 6))
front = {old: 11}
back = {key: 7}
chain = ChainMap(front, back)
try:
"""
END = """except MemoryError:
    print('MemoryError')
print(len(front))
print(key in front)
print(front[old])
print(back[key])
"""
OPERATIONS = {
    "contains": ("    print(key in chain)", "unsigned", "void *d, size_t k", "d, k"),
    "getitem": ("    print(chain[key])", "size_t", "void *d, size_t k, unsigned kind", "d, k, kind"),
    "try_get": ("    print(chain.get(key, 29))", "unsigned", "void *d, size_t k, size_t *out", "d, k, out"),
    "setitem": ("    chain[key] = 13\n    print('stored')", "void", "void *d, size_t k, size_t v", "d, k, v"),
}
WRAPPER = """
#include <stdlib.h>
#include <unistd.h>
static int active, refused;
static int refuse(void) {
    if (!active || refused) return 0;
    refused = 1;
    (void)write(2, "refused\\n", 8);
    return 1;
}
void *__real_malloc(size_t);
void *__real_realloc(void *, size_t);
void *__wrap_malloc(size_t n) { return refuse() ? NULL : __real_malloc(n); }
void *__wrap_realloc(void *p, size_t n) { return refuse() ? NULL : __real_realloc(p, n); }
"""


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--compiler-revision", required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("run with CPython 3.14.5")
    tool, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
    report = {"compiler_revision": args.compiler_revision, "imre_sha256": digest(tool),
              "runtime_sha256": digest(runtime), "operations": {}}
    env = dict(os.environ)
    env.pop("IMRE_REFUSE_CHAINMAP_KEY", None)
    for name, (stmt, returns, params, call) in OPERATIONS.items():
        symbol = "imre_chainmap_" + name
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            entry, hook, binary = (work / p for p in ("case.py", "wrap.c", "case"))
            entry.write_text(SETUP + stmt + "\n" + END)
            invoke = f"__real_{symbol}({call});"
            hook.write_text(WRAPPER + f"{returns} __real_{symbol}({params});\n"
                            + f"{returns} __wrap_{symbol}({params}) {{\n"
                            + "active = getenv(\"IMRE_REFUSE_CHAINMAP_KEY\") != NULL;\n"
                            + (invoke if returns == "void" else f"{returns} result = {invoke}")
                            + "\nactive = 0;\n" + ("" if returns == "void" else "return result;") + "\n}\n")
            expected = subprocess.run([sys.executable, "-B", entry], cwd=work, env=env, capture_output=True,
                                      text=True, check=True)
            modes = report["operations"][name] = {}
            for level in (0, 3):
                flags = [flag for s in (symbol, "malloc", "realloc") for flag in ("-C", "link-arg=-Wl,--wrap=" + s)]
                built = subprocess.run([tool, "build", entry, "--no-cache", "--runtime", runtime, "--clang", clang,
                                        "--opt-level", str(level), *flags, "-C", "link-arg=" + str(hook), "-o", binary],
                                       cwd=work, text=True, capture_output=True)
                if built.returncode:
                    raise RuntimeError(f"{name} O{level}: {built.stderr or built.stdout}")
                seen = {}
                for label, run_env in (("baseline", env), ("refusal", {**env, "IMRE_REFUSE_CHAINMAP_KEY": "1"})):
                    run = subprocess.run([binary], cwd=work, env=run_env, text=True, capture_output=True, timeout=20)
                    want = ((expected.stdout, expected.stderr) if label == "baseline" else
                            ("MemoryError\n1\nFalse\n11\n7\n", "refused\n"))
                    if (run.returncode, run.stdout, run.stderr) != (0, *want):
                        raise AssertionError((name, level, label, run.returncode, run.stdout, run.stderr, want))
                    seen[label] = {"stdout": run.stdout, "stderr": run.stderr, "exit": run.returncode}
                modes[f"O{level}"] = seen
                print(f"O{level} {name}: matched CPython; MemoryError preserved both mappings", flush=True)
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
