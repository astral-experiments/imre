import subprocess
from sys import argv


def dispositions(probe: list[str], restore: bool) -> bytes:
    return subprocess.check_output(probe, restore_signals=restore)


probe = [argv[4], "dispositions"]
assert dispositions(probe, False) == b"1 1\n"
assert dispositions(probe, True) == b"0 0\n"
assert subprocess.check_output(probe) == b"0 0\n"
with subprocess.Popen(probe, stdout=subprocess.PIPE, restore_signals=False,
                      pass_fds=(1,), process_group=0, umask=0o077) as process:
    out, _ = process.communicate()
    assert out == b"1 1\n"

# Raising the signal verifies the child's behavior, and launching an inheriting
# child last catches any change to the parent's dispositions during a launch.
assert subprocess.call([argv[4]], restore_signals=True) == -13
assert subprocess.call([argv[4]], restore_signals=False) == 77
print("child read both dispositions and raised SIGPIPE")
