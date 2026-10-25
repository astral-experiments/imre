from pathlib import Path
import shutil
import subprocess


def unbound(text: str) -> int:
    try:
        value: int = int(text)
    except ValueError:
        pass
    return value


# Use the actual producers: the compiler's catch IDs must agree with the
# linked runtime that raises and matches each exception.
try:
    subprocess.run(["/bin/sh", "-c", "exit 7"], check=True)
except (UnicodeEncodeError, NameError, LookupError, shutil.Error, OSError):
    raise AssertionError("child exit matched an unrelated handler")
except subprocess.CalledProcessError as child:
    print(f"child exit {child.returncode}")

try:
    subprocess.run(["/bin/sh", "-c", "exec sleep 10"], timeout=0)
except (subprocess.CalledProcessError, UnicodeEncodeError, UnboundLocalError):
    raise AssertionError("timeout matched an unrelated handler")
except subprocess.SubprocessError:
    print("timeout through subprocess base")

try:
    subprocess.Popen(["./no-such-executable-for-crossed-catches"])
except (shutil.Error, subprocess.SubprocessError, UnicodeEncodeError):
    raise AssertionError("missing executable matched an unrelated handler")
except FileNotFoundError:
    print("missing executable")

try:
    Path("encoding.txt").write_text("é", encoding="ascii")
except (subprocess.SubprocessError, NameError, LookupError, shutil.Error):
    raise AssertionError("encoding failure matched an unrelated handler")
except UnicodeEncodeError:
    print("encoding")

try:
    unbound("invalid")
except (subprocess.SubprocessError, LookupError, UnicodeEncodeError):
    raise AssertionError("unbound local matched an unrelated handler")
except NameError:
    print("unbound local through name base")

mapping = {"present": 1}
try:
    print(mapping["absent"])
except (subprocess.SubprocessError, NameError, UnicodeEncodeError):
    raise AssertionError("key lookup matched an unrelated handler")
except LookupError:
    print("key through lookup base")

try:
    shutil.copyfile("sentinel.txt", "sentinel.txt")
except (subprocess.SubprocessError, UnicodeEncodeError, NameError, LookupError):
    raise AssertionError("same file matched an unrelated handler")
except shutil.SameFileError:
    print("same file")
