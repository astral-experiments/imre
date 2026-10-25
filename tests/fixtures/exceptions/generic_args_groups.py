import asyncio


class BadDisplay(Exception):
    def __init__(self, label: str) -> None:
        super().__init__(label)
        self.label = label

    def __repr__(self) -> str:
        raise ValueError(self.label)


async def fail() -> None:
    raise ValueError("child")


async def bad() -> None:
    raise BadDisplay("repr failed")


async def ordinary() -> None:
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(fail())
    except ExceptionGroup as failures:
        print(repr(failures))
        print(repr(failures.args))
        saved = failures.args
        print(isinstance(saved[1], list))
        print(isinstance(saved[1], dict))
        try:
            raise failures.exceptions[0] from failures
        except ValueError as child:
            print(repr(saved))
            print(repr(child.args))
            parent = child.__cause__
            assert parent is not None
            print(repr(parent.args))


async def failed_repr() -> None:
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(bad())
    except ExceptionGroup as failures:
        try:
            repr(failures.args)
        except ValueError as error:
            print(str(error))


asyncio.run(ordinary())
asyncio.run(failed_repr())
