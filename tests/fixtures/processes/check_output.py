from pathlib import Path
from subprocess import CalledProcessError, check_output

print(repr(check_output([Path("/bin/sh"), "-c", "printf 'x\\r\\ny\\rz'"], encoding="utf8")))
command: list[str | Path] = [Path("/bin/sh"), "-c", "printf 'captured'; printf 'inherited' >&2; exit 1"]
try:
    check_output(command, encoding="utf-8")
except CalledProcessError as error:
    print(error.returncode)
    print(error.stderr is None)
    print(error.output)
    print(error.stdout)
    print(str(error))
    command.append("argument")
    print(len(error.cmd))
    print(repr(error.cmd[0]))
    print(str(error))
