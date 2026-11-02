import os
import subprocess


def child_environment() -> bytes:
    return subprocess.run(["/usr/bin/env"], capture_output=True, check=True).stdout


saved = dict(os.environb)
os.environ.clear()
assert child_environment() == b""
os.environ.update({"IMRE_RESTORE": "text λ"})
text = dict(os.environ)
assert child_environment() == b"IMRE_RESTORE=text \xce\xbb\n"
os.environb.clear()
os.environ.update(text)
assert dict(os.environ) == text

try:
    os.environb.update({b"IMRE_RAW": b"\xff", b"BAD=NAME": b"unused", b"LATE": b"value"})
except ValueError:
    pass
else:
    raise AssertionError("invalid environment name was accepted")
assert child_environment() == b"IMRE_RESTORE=text \xce\xbb\nIMRE_RAW=\xff\n"
os.environb.clear()
os.environb.update(saved)
assert dict(os.environb) == saved
assert b"IMRE_RAW_BASELINE=\xff\n" in child_environment()
print("restored")
