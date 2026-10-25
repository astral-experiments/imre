import asyncio
from pathlib import Path
from subprocess import DEVNULL, PIPE
from tempfile import TemporaryDirectory


async def partner(cwd: Path, name: str, other: str) -> str:
    script = 'touch "$1"; n=0; while ! test -e "$2"; do n=$((n+1)); test "$n" -lt 200 || exit 91; sleep .01; done; printf %s "$1"; printf stderr >&2'
    options = ["-c", script, "barrier", name, other]
    child = await asyncio.create_subprocess_exec(
        "/bin/sh", *options, cwd=cwd, stdout=PIPE, stderr=PIPE
    )
    stdout, stderr = await child.communicate()
    assert stderr == b"stderr"
    assert child.returncode == 0
    assert await child.wait() == 0
    return stdout.decode()


async def main() -> None:
    with TemporaryDirectory() as directory:
        path = Path(directory)
        async with asyncio.TaskGroup() as group:
            left = group.create_task(partner(path, "left", "right"))
            right = group.create_task(partner(path, "right", "left"))
        print(f"{left.result()} {right.result()}")
        child = await asyncio.create_subprocess_exec("/bin/sh", "-c", "exit 7", stdout=DEVNULL)
        assert await child.wait() == 7
        stdout, stderr = await child.communicate()
        assert stdout is None
        assert stderr is None


asyncio.run(main())
