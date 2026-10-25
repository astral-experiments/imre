import asyncio
from pathlib import Path
from signal import SIGTERM
from subprocess import DEVNULL
from tempfile import TemporaryDirectory


async def worker(directory: Path, name: str, cleaned: list[str]) -> int:
    child = await asyncio.create_subprocess_exec(
        "/bin/sleep", ".15" if name == "first" else ".4", stdout=DEVNULL, stderr=DEVNULL
    )
    (directory / name).write_text("ready", encoding="utf-8")
    try:
        return await child.wait()
    finally:
        await child.wait()
        cleaned.append(name)
        (directory / ("released-" + name)).write_text("closed", encoding="utf-8")


async def main() -> None:
    with TemporaryDirectory() as temporary:
        directory = Path(temporary)
        command = 'i=0; while [ ! -f first ] || [ ! -f second ]; do i=$((i+1)); [ $i -lt 200 ] || exit 92; sleep .01; done; kill -TERM "$PPID"'
        signaler = await asyncio.create_subprocess_exec(
            "/bin/sh", "-c", command, cwd=directory, stdout=DEVNULL, stderr=DEVNULL
        )
        cleaned: list[str] = []
        try:
            await asyncio.gather(*[worker(directory, name, cleaned) for name in ["first", "second"]])
        except asyncio.CancelledError:
            print(repr(cleaned))
        print(await signaler.wait())
        waiter = await asyncio.create_subprocess_exec(
            "/bin/sh", "-c", "i=0; while [ ! -f released-second ]; do i=$((i+1)); [ $i -lt 200 ] || exit 93; sleep .01; done",
            cwd=directory, stdout=DEVNULL, stderr=DEVNULL,
        )
        print(await waiter.wait())
        print(repr(sorted(cleaned)))


loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
task = asyncio.ensure_future(main())
loop.add_signal_handler(SIGTERM, task.cancel)
try:
    loop.run_until_complete(task)
finally:
    loop.close()
