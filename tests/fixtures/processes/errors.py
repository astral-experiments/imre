import subprocess
from pathlib import Path
from subprocess import CalledProcessError, SubprocessError
from sys import argv


def outcome(command: list[str], cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    try:
        subprocess.run(command, capture_output=True, encoding="utf-8", check=True, cwd=cwd, env=env)
    except CalledProcessError:
        return "CalledProcessError"
    except UnicodeDecodeError:
        return "UnicodeDecodeError"
    except FileNotFoundError:
        return "FileNotFoundError"
    except PermissionError:
        return "PermissionError"
    except NotADirectoryError:
        return "NotADirectoryError"
    except IndexError:
        return "IndexError"
    except ValueError:
        return "ValueError"
    except OSError:
        return "OSError"
    raise AssertionError("child unexpectedly succeeded")


def checked(command: list[str]) -> Exception:
    try:
        subprocess.run(command, capture_output=True, encoding="utf-8", check=True)
    except CalledProcessError as error:
        return error
    raise AssertionError("child unexpectedly succeeded")


def invalid(command: list[str], cwd: str, env: dict[str, str]) -> None:
    try:
        subprocess.run(command, cwd=cwd, env=env, capture_output=True, encoding="utf-8")
    except ValueError as error:
        print("ValueError: " + str(error))
    except IndexError as error:
        print("IndexError: " + str(error))


command = [argv[1], "-I", argv[2], "status", "9"]
result = subprocess.run(command, capture_output=True, encoding="utf-8", check=False)
print(f"{result.returncode} {result.stdout!r} {result.stderr!r}")
try:
    result.check_returncode()
except SubprocessError as error:
    print(f"{error.returncode} {repr(error.cmd) == repr(command)} {error.stdout!r} {error.output!r} {error.stderr!r}")
    print(str(error))
    command.append("mutated command")
    print(str(error))
saved = checked(command)
print(f"{isinstance(saved, CalledProcessError)} {isinstance(saved, SubprocessError)} {isinstance(saved, OSError)}")
print(f"{saved.returncode} {saved.cmd[-1]}")
saved.cmd.append("mutated through exception")
print(command[-1])
print(str(saved))
try:
    raise saved
except Exception as again:
    print(f"{again.returncode} {again.stdout == again.output}")

print(outcome([]))
print(outcome([""], env={"PATH": "directory"}))
print(outcome(["./missing-executable"]))
print(outcome(["./not-executable"]))
print(outcome(["./executable-text"]))
print(Path("fallback.txt").exists())
print(outcome(["./plain-file/child"]))
print(outcome(command, Path("missing-directory")))
print(outcome(command, Path("plain-file")))
print(outcome(["bad\0executable"]))
print(outcome([argv[1], "bad\0argument"]))
print(outcome(command, Path("bad\0directory")))
print(outcome(command, env={"BAD=KEY": "value"}))
print(outcome(command, env={"BAD\0KEY": "value"}))
print(outcome(command, env={"KEY": "bad\0value"}))
# An explicit cwd selects fork/exec's validation order for simultaneous errors.
invalid([argv[1], "bad\0argument"], ".", {"A=B": "value"})
invalid([argv[1]], "bad\0directory", {"A=B": "value"})
invalid([argv[1]], ".", {"FIRST": "bad\0value", "A=B": "value"})
invalid([], "bad\0directory", {"FIRST": "bad\0value", "A=B": "value"})
print(outcome([argv[1], "-I", argv[2], "invalid", "1"]))
print(outcome([argv[1], "-I", argv[2], "invalid", "2"]))
print(Path("completed.txt").read_text(encoding="utf-8"))
signal = subprocess.run([argv[1], "-I", argv[2], "signal"], capture_output=True, encoding="utf-8")
print(signal.returncode)
signal_error = checked([argv[1], "-I", argv[2], "signal"])
print(f"{signal_error.returncode} {signal_error}")

other = ValueError("ordinary error")
try:
    print(other.returncode)
except AttributeError:
    print("no returncode")
try:
    print(repr(other.cmd))
except AttributeError:
    print("no cmd")
try:
    print(other.stdout)
except AttributeError:
    print("no stdout")
try:
    print(other.output)
except AttributeError:
    print("no output")
try:
    print(other.stderr)
except AttributeError:
    print("no stderr")
