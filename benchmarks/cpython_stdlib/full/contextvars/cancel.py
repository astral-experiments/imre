import asyncio
from contextvars import ContextVar
from signal import SIGTERM
from subprocess import DEVNULL

binding: ContextVar[int] = ContextVar("binding", default=-1)


async def interrupted() -> int:
    token = binding.set(13)
    child = await asyncio.create_subprocess_exec(
        "/bin/sh", "-c", 'sleep .05; kill -TERM "$PPID"; sleep .05',
        stdout=DEVNULL, stderr=DEVNULL,
    )
    try:
        await child.wait()
    except asyncio.CancelledError:
        print(binding.get())
        binding.reset(token)
        print(binding.get())
        return await child.wait()
    raise AssertionError("expected cancellation")


binding.set(3)
loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
task = asyncio.ensure_future(interrupted())
binding.set(5)
loop.add_signal_handler(SIGTERM, task.cancel)
try:
    print(loop.run_until_complete(task))
    print(binding.get())
finally:
    loop.close()
