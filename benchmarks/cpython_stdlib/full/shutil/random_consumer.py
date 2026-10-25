"""Compare the pinned shutil random consumer and shared draws against CPython 3.14.5."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_native import NativeCompileError, Toolchain

def digest(data):
    return hashlib.sha256(data).hexdigest()

def programs():
    source = (HERE / "upstream/Lib/test/test_shutil.py").read_bytes()
    assert hashlib.sha1(b"blob %d\0" % len(source) + source).hexdigest() == "fb1a7d876a617e17bcecbb8d8b654a2a87dafe48"
    text = source.decode()
    fn, = (n for n in ast.parse(text).body if isinstance(n, ast.FunctionDef) and n.name == "write_test_file")
    draw, = (n for n in ast.walk(fn) if isinstance(n, ast.Call) and ast.unparse(n.func) == "random.choice")
    expression = ast.get_source_segment(text, draw)
    imports = "import random\nimport string\n"
    seed = "random.seed(8675309)\n"
    yield "shutil-expression", imports + seed + f"for i in range(10000):\n    print({expression})\n"
    # Keep the original definition before executable module statements.
    yield "shutil-write-test-file", ("import os\n" + imports + ast.get_source_segment(text, fn)
                                    + "\n" + seed + 'write_test_file("fixture.bin", 4096)\n')
    yield "bits-and-floats", """import random
random.seed(-1267650600228229401496703205393)
for i in range(1300):
    print(random.getrandbits(32))
for bits in (0, 1, 31, 32, 33, 63, 64, 65, 127, 256):
    print(random.getrandbits(bits))
    print(int(random.random() * 9007199254740992.0))
"""
    yield "functools-ranges", """from random import choice, randint, randrange, seed
seed(407)
for i in range(200):
    print(choice(range(5)))
    print(choice(range(29, -17, -4)))
    print(randint(0,10000))
    print(randrange(-234,739,12))
    print(randrange(1267650600228229401496703205393))
"""
    yield "sequence-choice", """import random
random.seed("μa")
texts = ["alpha", "beta", "gamma"]
for i in range(80):
    print(random.choice(seq="🐞λx"))
    print(random.choice(b"abc"))
    print(random.choice(texts))
random.seed(b"abc")
print(random.getrandbits(128))
"""
    yield "single-evaluation", """from random import choice, seed

def items(calls: list[int]) -> list[str]:
    calls.append(1)
    return ["one", "two", "three"]

seed(123)
calls: list[int] = []
print(choice(items(calls)))
print(len(calls))
for i in range(10):
    print(choice([50]))
    print(choice([25, 75]))
"""
    yield "error-state", """import random
random.seed(125)
empty: list[int] = []
try:
    random.choice(empty)
except IndexError as error:
    print(str(error))
try:
    random.choice("")
except IndexError as error:
    print(str(error))
try:
    random.choice(range(10, -1))
except IndexError as error:
    print(str(error))
try:
    random.choice(range(0, 4, 0))
except ValueError as error:
    print(str(error))
try:
    random.choice(range(1267650600228229401496703205376))
except OverflowError as error:
    print(str(error))
try:
    random.getrandbits(-1)
except ValueError as error:
    print(str(error))
try:
    random.getrandbits(1267650600228229401496703205376)
except OverflowError as error:
    print(str(error))
try:
    random.randint(7,-8)
except ValueError as error:
    print(str(error))
try:
    random.randrange(4, 80,-2)
except ValueError as error:
    print(str(error))
print(random.getrandbits(80))
"""

def observe(cmd, root):
    root.mkdir()
    run = subprocess.run(cmd, cwd=root, capture_output=True, timeout=45)
    fixture = root / "fixture.bin"
    return {"returncode":run.returncode,"stdout_sha256":digest(run.stdout),"stdout_lines":len(run.stdout.splitlines()),
            "stderr":run.stderr.decode(errors="replace"),"fixture_sha256":digest(fixture.read_bytes()) if fixture.exists() else None}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    for n in ("imre","runtime","clang","result"):
        p.add_argument("--"+n, type=lambda s: Path(s).resolve(), required=True)
    args=p.parse_args()
    if sys.version_info[:3] != (3,14,5): p.error("run with CPython 3.14.5")
    report={"compiler_sha256":digest(args.imre.read_bytes()),"runtime_sha256":digest(args.runtime.read_bytes()),"cases":{}}
    failed=False
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp)
        for name, source in programs():
            script=root/(name+".py"); script.write_text(source)
            expected=observe([sys.executable,"-IB",str(script)],root/(name+"-cpython")); assert expected["returncode"] == 0, expected
            row={"source_sha256":digest(source.encode()),"cpython":expected,"levels":{}}
            report["cases"][name]=row
            for level in (0,3):
                build=root/f"{name}-build-{level}";build.mkdir()
                try:
                    exe=Toolchain(args.imre,args.runtime,args.clang,level).build(source.encode(),build)
                except NativeCompileError as exc:
                    result={"status":"compile-blocked","diagnostic":str(exc).replace(str(build) + "/", "")}
                else:
                    actual=observe([str(exe)],root/f"{name}-run-{level}")
                    result={"status":"pass" if actual==expected else "failed","observed":actual}
                row["levels"][str(level)]=result
                failed |= (result["status"] == "failed" or
                           (result["status"] == "compile-blocked" and name != "shutil-write-test-file"))
                print(f"{name} O{level}: {result['status']}",flush=True)
    args.result.write_text(json.dumps(report,indent=2)+"\n")
    return int(failed)

if __name__=="__main__":
    sys.exit(main())
