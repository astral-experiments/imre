from pathlib import Path
import subprocess
from subprocess import getoutput, getstatusoutput


def decoded(encoding: str | None, errors: str | None) -> str:
    return subprocess.getoutput("printf '\\377a'", encoding=encoding, errors=errors)


print(getoutput("echo xyzzy"))
print(getstatusoutput("printf out; printf err >&2; exit 7"))
print(getstatusoutput("kill -TERM $$"))
print(getstatusoutput(":"))
print(repr(getoutput("printf 'a\\r\\nb\\r\\n\\n'")))
print(repr(getoutput("printf '\\n\\n'")))
print(getstatusoutput(["printf '%s:%s' \"$0\" \"$1\"", "name", "arg"]))
print(getoutput(b"printf bytes"))
print(getoutput(cmd=("printf '%s' \"$0\"", "tuple")))
print(len(getoutput("head -c 200000 /dev/zero; printf '\\n'")))


print(repr(decoded("latin-1", None)))
print(repr(decoded("ascii", "replace")))
print(repr(decoded(None, "ignore")))
print(repr(decoded("utf-8", "backslashreplace")))
print(getoutput("printf '\\357\\273\\277ok\\n'", encoding="utf-8-sig"))
try:
    decoded(None, None)
except UnicodeDecodeError as exc:
    print(str(exc))
print(getoutput("printf valid", errors="missing_handler"))
try:
    decoded("ascii", "missing_handler")
except LookupError as exc:
    print(str(exc))

# The encoding is looked up before the shell can modify the filesystem.
try:
    getstatusoutput("printf bad > shell-before-lookup", encoding="missing-codec")
except LookupError as exc:
    print(str(exc))
print(Path("shell-before-lookup").exists())
