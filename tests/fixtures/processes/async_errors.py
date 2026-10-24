import asyncio


class ToolError(Exception):
    pass


class RuleError(ToolError):
    pass


async def check() -> None:
    raise RuleError("bad output")


async def compare() -> None:
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(check())
    except ExceptionGroup as failures:
        raise failures.exceptions[0] from failures


try:
    asyncio.run(compare())
except ToolError as error:
    print(str(error))
    print(isinstance(error, RuleError))
    print(isinstance(error, ValueError))
    parent = error.__cause__
    assert parent is not None
    print(str(parent))
    print(str(parent.exceptions[0]))
