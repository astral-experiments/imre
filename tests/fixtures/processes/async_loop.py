import asyncio
from signal import SIGINT, SIGTERM
from subprocess import DEVNULL


async def interrupted() -> int:
    child = await asyncio.create_subprocess_exec(
        "/bin/sh", "-c", 'sleep .05; kill -TERM "$PPID"; sleep .05',
        stdout=DEVNULL, stderr=DEVNULL,
    )
    try:
        await child.wait()
    except asyncio.CancelledError:
        return await child.wait()
    raise AssertionError("the signal should have cancelled this task")


loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
task = asyncio.ensure_future(interrupted())
for signal in [SIGINT, SIGTERM]:
    loop.add_signal_handler(signal, task.cancel)
try:
    print(loop.run_until_complete(task))
    print(task.cancel())
finally:
    loop.close()
