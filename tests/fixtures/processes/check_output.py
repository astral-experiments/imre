from pathlib import Path
from subprocess import PIPE, CalledProcessError, check_output, run


def checked(text: bool) -> None:
    args = ["/bin/sh", "-c", "printf failed; exit 1"]
    if text:
        check_output(args, encoding="utf-8")
    else:
        run(args, stdout=PIPE, check=True)


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


for text in [True, False]:
    try:
        checked(text)
    except CalledProcessError as error:
        print(repr(error.stdout))
        print(repr(error.output))
        print(repr(error.stderr))
