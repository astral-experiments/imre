from pathlib import Path
import shutil
import subprocess
import sys
import warnings

marker = Path("warning-child-ran")
command = [sys.argv[1], "-c", "open('warning-child-ran', 'w').close()"]
warnings.simplefilter("error", RuntimeWarning)
try:
    subprocess.call(command, pass_fds=(1,), close_fds=False)
except RuntimeError:
    raise AssertionError("RuntimeError caught RuntimeWarning")
except RuntimeWarning as exc:
    print(str(exc))
assert not marker.exists()

# The warning precedes conversion of individual descriptors and redirected files.
with open("closed-stream", "w") as closed:
    pass
try:
    subprocess.Popen(command, stdout=closed, pass_fds=[-1], close_fds=False)
except Warning as exc:
    print(str(exc))
try:
    subprocess.Popen(command, stdout=subprocess.STDOUT, pass_fds={1}, close_fds=False)
except ValueError as exc:
    print(str(exc))
assert not marker.exists()

# Suppression must match the category, message, module and line.
warnings.filterwarnings("ignore", "PASS_FDS", RuntimeWarning, "subprocess$", 862)
print(subprocess.call(command, pass_fds={1}, close_fds=False))
assert marker.exists()
true_path = shutil.which("true")
assert true_path is not None
print(subprocess.call([true_path], pass_fds=(), close_fds=False))
print(subprocess.call([true_path], pass_fds=(1,), close_fds=True))

warnings.simplefilter("error", Warning)
warnings.filterwarnings("ignore", "pass_fds", RuntimeWarning, "subprocess$", 863)
warnings.simplefilter("ignore", ResourceWarning)
try:
    subprocess.call(command, pass_fds=(1,), close_fds=False)
except subprocess.SubprocessError:
    raise AssertionError("SubprocessError caught RuntimeWarning")
except Warning as exc:
    print(str(exc))
try:
    raise RuntimeError("ordinary runtime error")
except RuntimeWarning:
    raise AssertionError("RuntimeWarning caught RuntimeError")
except RuntimeError as exc:
    print(str(exc))
