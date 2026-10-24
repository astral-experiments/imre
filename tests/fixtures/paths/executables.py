import os.path
import shutil
import sys
import sysconfig
from pathlib import Path


print(sys.platform)
print(Path(sysconfig.get_path("scripts")).name)
for first in ["", ".", "/", "//", "./a//"]:
    for tail in ["", ".", "/absolute", "relative//tail"]:
        print(os.path.join(first, tail))
print(os.path.join(Path("a/b"), "c", "", "../..", ""))
for path in ["", ".", "input.txt", "input.txt/", "file-link", "dangling-link", "bad\0name"]:
    print(os.path.exists(path))
for program in ["./command", "./input.txt", "./tree", "./missing", "tree/../command", "relar-missing-command"]:
    print(repr(shutil.which(program)))
print("missing executable", sys.stderr)
