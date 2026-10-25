import subprocess
import warnings


def child(label: str) -> None:
    proc = subprocess.Popen(["/bin/sh", "-c", "exit 0"], stdout=subprocess.PIPE)
    proc.stdout.read()
    proc.stdout.close()
    print(f"{label} {proc.pid}")


def mark(label: str, value: int) -> int:
    print(label)
    return value


def run() -> None:
    child("initial")
    warnings.filterwarnings("always", category=ResourceWarning, append=True)
    child("after-default-ignore")
    warnings.resetwarnings()
    child("default-after-reset")
    warnings.simplefilter("ignore")
    warnings.filterwarnings("always", "SUBPROCESS [0-9]+ IS STILL", ResourceWarning, r"subprocess\Z")
    child("matches")
    warnings.filterwarnings("ignore", message="process")
    child("message-must-start")
    warnings.filterwarnings("ignore", module="SUBPROCESS")
    child("module-is-case-sensitive")
    warnings.filterwarnings("ignore", category=FutureWarning)
    child("other-category")
    warnings.simplefilter("ignore", ResourceWarning, 10**75)
    child("other-line")
    warnings.filterwarnings("ignore", module="subprocess", lineno=1139)
    child("exact-line")
    warnings.filterwarnings("always", "SUBPROCESS [0-9]+ IS STILL", ResourceWarning, r"subprocess\Z", append=True)
    child("append-duplicate")
    warnings.filterwarnings("always", "SUBPROCESS [0-9]+ IS STILL", ResourceWarning, r"subprocess\Z")
    child("prepend-duplicate")
    warnings.filterwarnings("ignore", module=r"subprocess$", append=mark("append first", 0), lineno=mark("line second", 0))
    child("source-evaluation-order")
    try:
        warnings.filterwarnings(action="bad", category=int, lineno=-1)
    except ValueError as error:
        print(str(error))
    try:
        warnings.filterwarnings("always", message=mark("message third", 1), lineno=mark("line fourth", -1))
    except TypeError as error:
        print(str(error))
    warnings.resetwarnings()
    child("reset-again")
    subprocess.Popen(["/bin/sh", "-c", "exit 0"]).wait()


run()
