from facade import Failed, Result, environ, execute
from sys import argv


class Local:
    def __init__(self) -> None:
        pass

    def run(self, value: int) -> int:
        return value + 1


def invoke(command: list[str]) -> Result[str]:
    return execute(command, capture_output=True, encoding="utf-8", env=environ.copy())


print(Local().run(41))
print(invoke([argv[1], "-I", argv[2], "echo", "imported aliases"]).stdout)
try:
    execute([argv[1], "-I", argv[2], "status", "3"], capture_output=True, encoding="utf-8", check=True)
except Failed as error:
    print(f"{error.returncode} {error.stdout!r}")
