"""Compare custom archive registration, cwd restoration, and returned filenames."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
from copyfile import build

SOURCE = b'''import os
import shutil

def create(base: str, directory: str, *, dry_run: int, logger: None, owner: str | None, group: str | None, label: str = "default") -> str:
    print(os.getcwd())
    print(base)
    print(directory)
    print(dry_run)
    print(owner if owner is not None else "<none>")
    print(group if group is not None else "<none>")
    print(label)
    return base + ".custom"

def fail(base: str, directory: str, *, dry_run: int, logger: None, owner: str | None, group: str | None) -> str:
    print(os.getcwd())
    raise RuntimeError("original archiver exception")

def rooted(base: str, directory: str, *, dry_run: int, logger: None, owner: str | None, group: str | None, root_dir: str | None = None) -> str:
    print(os.getcwd())
    print(root_dir if root_dir is not None else "<no root>")
    return base + ".rooted"

def capture(base: str, /, directory: str, *, dry_run: int, logger: None, owner: str | None, group: str | None, **labels: str) -> str:
    print("pack kwargs")
    print(labels.get("base", "absent"))
    print(labels.get("labels", "absent"))
    print(len(labels))
    labels["used"] = "once"
    return base + ".captured"

def extract(source: str, dest: str, /, **weights: int) -> None:
    print("unpack kwargs")
    print(weights.get("source", -1))
    print(weights.get("weights", -1))
    print(len(weights))
    weights["edited"] = 9

try:
    shutil.register_archive_format("xxx", 1)
except TypeError as error:
    print(str(error))
try:
    shutil.register_archive_format("xxx", create, 1)
except TypeError as error:
    print(str(error))
try:
    shutil.register_archive_format("xxx", create, [(1,2,3)])
except TypeError as error:
    print(str(error))
shutil.register_archive_format("xxx", create, [(1,2)], "extra keys checked on call")
try:
    shutil.make_archive("base", "xxx")
except TypeError as error:
    print(str(error))
shutil.register_archive_format("xxx", create, [("label","one"),("label","last")], "custom pack")
for name, description in shutil.get_archive_formats():
    if name == "xxx":
        print(description)
print(shutil.make_archive("base", "xxx"))
print(shutil.make_archive("base", "xxx", root_dir="content", base_dir="leaf", owner="alice", group="staff"))
print(os.getcwd())
print(shutil.make_archive("base", "xxx", root_dir="content", dry_run=2))
print(os.getcwd())
shutil.register_archive_format("xxx", fail)
try:
    shutil.make_archive("base", "xxx", root_dir="content")
except RuntimeError as error:
    print(str(error))
print(os.getcwd())
shutil.unregister_archive_format("xxx")
try:
    shutil.unregister_archive_format("xxx")
except KeyError as error:
    print(str(error))
shutil.register_archive_format("rooted", rooted)
shutil.register_archive_format("same_function", rooted)
rooted.supports_root_dir = True
print(shutil.make_archive("relative", "rooted", root_dir="switched"))
print(shutil.make_archive("relative", "same_function"))
rooted.supports_root_dir = False
print(shutil.make_archive("relative", "same_function", root_dir="switched"))
print(os.getcwd())
shutil.unregister_archive_format("rooted")
shutil.unregister_archive_format("same_function")

shutil.register_archive_format("capture", capture, [("base", "shadow"), ("labels", "outer")])
print(shutil.make_archive("one", "capture"))
print(shutil.make_archive("two", "capture"))
shutil.register_archive_format("capture", capture)
print(shutil.make_archive("three", "capture"))
shutil.register_archive_format("capture", capture, [("directory", "duplicate")])
try:
    shutil.make_archive("four", "capture")
except TypeError as error:
    print(str(error))
shutil.unregister_archive_format("capture")

shutil.register_unpack_format("weights", [".weights"], extract, [("source", 7), ("weights", 2)])
shutil.unpack_archive("example.weights", "out")
shutil.unpack_archive("example.weights", "out")
shutil.unregister_unpack_format("weights")
'''

def main():
    p=argparse.ArgumentParser(description=__doc__)
    for n in ("imre","runtime","clang"):
        p.add_argument("--"+n,type=lambda s:Path(s).resolve(),required=True)
    args=p.parse_args()
    if sys.version_info[:3]!=(3,14,5) or os.name!="posix":p.error("requires POSIX CPython 3.14.5")
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp);(root/"content/leaf").mkdir(parents=True);(root/"switched").mkdir()
        script=root/"pack.py";script.write_bytes(SOURCE)
        def run(cmd):
            proc=subprocess.run(cmd,cwd=root,capture_output=True,timeout=40)
            assert proc.returncode==0 and not proc.stderr,(cmd,proc.returncode,proc.stdout,proc.stderr)
            return proc.stdout
        expected=run([sys.executable,"-IB",str(script)])
        assert b"The 1 object is not callable" in expected and b"original archiver exception" in expected
        assert expected.count(os.fsencode(root/"content")+b"\n")==2
        assert b"switched\nrelative.rooted\n" in expected
        assert os.fsencode(root/"switched")+b"\n<no root>\n" in expected
        assert expected.count(b"pack kwargs\nshadow\nouter\n2\n")==2
        assert b"pack kwargs\nabsent\nabsent\n0\n" in expected
        assert b"capture() got multiple values for argument 'directory'\n" in expected
        assert expected.count(b"unpack kwargs\n7\n2\n2\n")==2
        for level in (0,3):
            exe=build(args,root,f"native-{level}",SOURCE,level);actual=run([str(exe)])
            assert actual==expected,(actual,expected)
            print(f"O{level}: registration, real cwd changes, dry run and original callback errors matched")

if __name__=="__main__":
    main()
