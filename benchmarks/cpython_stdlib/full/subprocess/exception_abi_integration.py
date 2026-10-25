"""Cross subprocess handlers with the published CSV and ImportError runtimes."""

import csv
import subprocess

try:
    subprocess.run(["/bin/sh", "-c", "exit 9"], check=True)
except (csv.Error, ImportError, ModuleNotFoundError):
    raise AssertionError("checked child exit matched CSV or import handler")
except subprocess.CalledProcessError as checked:
    print(f"child exit {checked.returncode}")

try:
    subprocess.run(["/bin/sh", "-c", "exec sleep 10"], timeout=0)
except (csv.Error, ImportError, subprocess.CalledProcessError):
    raise AssertionError("timed child matched CSV, import, or checked-exit handler")
except subprocess.TimeoutExpired:
    print("timeout")

reader = csv.reader(['"unterminated'], strict=True)
try:
    next(reader)
except (subprocess.SubprocessError, ImportError, OSError):
    raise AssertionError("CSV parser error matched an unrelated handler")
except csv.Error:
    print("CSV")

try:
    raise ModuleNotFoundError("missing module", name="absent")
except (subprocess.SubprocessError, csv.Error, NameError):
    raise AssertionError("missing module matched an unrelated handler")
except ImportError as missing:
    print(missing.name)

try:
    raise ImportError("import failed")
except (subprocess.SubprocessError, csv.Error, ModuleNotFoundError):
    raise AssertionError("base import error matched an unrelated handler")
except ImportError:
    print("import")
