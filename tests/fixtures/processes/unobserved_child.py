import subprocess
import time
import warnings


def child(command: str, observe: int) -> int:
    proc = subprocess.Popen(["/bin/sh", "-c", command], stdout=subprocess.PIPE)
    if observe == 1:
        proc.wait()
    if observe == 2:
        while proc.poll() is None:
            time.sleep(0.001)
    if observe == 3:
        proc.stdout.read()
    proc.stdout.close()
    return proc.pid


print("default")
print(child("exit 0", 3))
warnings.simplefilter("always", category=ResourceWarning)
print("live")
print(child("exec sleep 0.2", 0))
print("exited")
print(child("exit 0", 3))
print("waited")
print(child("exit 0", 1))
print("polled")
print(child("exit 0", 2))
warnings.simplefilter("ignore", RuntimeWarning)
print("other-category")
print(child("exit 0", 3))
warnings.simplefilter("ignore")
print("ignored")
print(child("exit 0", 3))
try:
    warnings.simplefilter("invented")
except ValueError as error:
    print(str(error))
time.sleep(0.25)
subprocess.Popen(["/bin/sh", "-c", "exit 0"]).wait()
