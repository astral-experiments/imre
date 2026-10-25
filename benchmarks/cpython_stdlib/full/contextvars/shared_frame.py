import asyncio
from contextvars import ContextVar

binding: ContextVar[int] = ContextVar("value", default=0)


async def read() -> int:
    return binding.get()


async def shared() -> int:
    before = binding.get()
    binding.set(70)
    inner = await asyncio.gather(read())
    print("inside")
    print(before)
    print(inner[0])
    print(binding.get())
    return binding.get()


async def main() -> None:
    for cancel_second in [False, True]:
        coro = shared()
        binding.set(11)
        first = asyncio.ensure_future(coro)
        binding.set(22)
        second = asyncio.ensure_future(coro)
        binding.set(33)
        if cancel_second:
            second.cancel()
        for task in [first, second]:
            try:
                result = await task
                print("result")
                print(result)
            except asyncio.CancelledError:
                print("cancelled")
            except RuntimeError as error:
                print(str(error))
        print("parent")
        print(binding.get())


asyncio.run(main())
