"""An eager display can suspend in its caller while retaining its iteration slots."""

import asyncio


async def observe(events: list[int], value: int) -> int:
    events.append(value)
    return value


async def collect(events: list[int]) -> list[int]:
    item = 99
    values = [
        await observe(events, item * 10 + inner)
        for item in range(3)
        if await observe(events, item)
        for inner in range(await observe(events, item + 1))
        if await observe(events, inner)
    ]
    events.append(item)
    return values


events: list[int] = []
for value in asyncio.run(collect(events)):
    print(value)
for event in events:
    print(event)
