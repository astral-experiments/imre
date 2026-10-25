import subprocess

def report(value: int | None) -> None:
    if value is None:
        print("None")
    else:
        print(value)

p = subprocess.Popen(["/bin/sleep", "30"])
alias = p
try:
    for code in [19, 2**90, -(2**90), 0]:
        alias.returncode = code
        report(p.returncode)
        report(p.poll())
        print(p.wait(timeout=0))
        # A known returncode avoids converting the signal or touching the live child.
        p.send_signal(2**100)
    p.returncode = None
    print(p.poll() is None)
    try:
        p.send_signal(12345)
    except OSError as exc:
        print("invalid live signal")
        report(exc.errno)
    p.terminate()
    print(p.wait())
    report(alias.returncode)
    alias.returncode = None
    report(p.poll())
    report(p.returncode)
finally:
    # Clear a manually assigned code if an earlier assertion failed.
    p.returncode = None
    p.kill()
    p.wait()

finished = subprocess.Popen(["/bin/sh", "-c", "exit 7"])
print(finished.wait())
finished.returncode = None
print(finished.wait(timeout=0))
report(finished.poll())
