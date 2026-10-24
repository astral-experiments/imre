import asyncio
from subprocess import DEVNULL


async def fail() -> None:
    raise ValueError("child failed")


async def main() -> None:
    child = await asyncio.create_subprocess_exec("/bin/sh", "-c", "sleep .05", stdout=DEVNULL, stderr=DEVNULL)
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(fail())
            await child.wait()
            raise AssertionError("parent should have been cancelled")
    except ExceptionGroup as error:
        print(str(error))
        print(str(error.exceptions[0]))
    print(await child.wait())


asyncio.run(main())
