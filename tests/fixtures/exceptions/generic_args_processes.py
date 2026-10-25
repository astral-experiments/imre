import subprocess


def show(exc: BaseException) -> None:
    print(repr(exc))
    print(repr(exc.args))


cmd = ["/bin/sh", "-c", "printf out; printf err >&2; exit 3"]
try:
    subprocess.run(cmd, capture_output=True, text=True, check=True)
except subprocess.CalledProcessError as e:
    show(e)
    cmd.append("mutated after raising")
    show(e)

p = subprocess.run(cmd, capture_output=True, text=True)
try:
    p.check_returncode()
except subprocess.SubprocessError as e:
    show(e)
p2 = subprocess.run(cmd, capture_output=True)
try:
    p2.check_returncode()
except BaseException as e:
    show(e)

try:
    subprocess.run(["/bin/sh", "-c", "printf 'a\\377'"], capture_output=True, text=True)
except UnicodeDecodeError as e:
    show(e)

try:
    subprocess.run(["/bin/sleep", "3"], capture_output=True, timeout=0.03)
except subprocess.TimeoutExpired as e:
    show(e)

try:
    subprocess.run(["/bin/sleep", "3"], timeout=0.03)
except subprocess.TimeoutExpired as e:
    print(repr(e.args[0]))
    budget = e.args[1]
    print(isinstance(budget, float))
    print(repr(budget) != repr(e.timeout))
    print(e.timeout == 0.03)
