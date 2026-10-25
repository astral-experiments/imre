import asyncio


class BadDisplay(Exception):
    def __init__(self, label: str) -> None:
        super().__init__(label)
        self.label = label

    def __repr__(self) -> str:
        raise ValueError(self.label)


class TracedDisplay(Exception):
    def __init__(self, events: list[str]) -> None:
        super().__init__()
        self.events = events

    def __repr__(self) -> str:
        self.events.append("rendered")
        return "TracedDisplay()"


class Envelope(Exception):
    def __init__(self, first, *rest):
        self.first = first
        super().__init__(first, *rest)


async def fail() -> None:
    raise ValueError("child")


async def bad() -> None:
    raise BadDisplay("repr failed")


async def traced(events: list[str]) -> None:
    raise TracedDisplay(events)


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
        member = saved[1][0]
        print(len(saved[1]))
        print(str(member))
        print(repr(member))
        print(bool(member))
        print(isinstance(member, Exception))
        print(isinstance(member, ValueError))
        print(isinstance(member, (str, TypeError)))
        print(isinstance(member, (str, BaseException)))
        try:
            len(member)
        except TypeError as error:
            print(str(error))
        try:
            member[0]
        except TypeError as error:
            print(str(error))
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
        try:
            str(failures.args)
        except ValueError as error:
            print(str(error))


async def mutations() -> None:
    events: list[str] = []
    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(traced(events))
    except ExceptionGroup as failures:
        print(len(events))
        print(str(failures.args))
        print(len(events))
        print(repr(failures.args))
        print(events)
        member = failures.args[1][0]
        limit = 10 ** 4300
        # Python invokes an earlier formatter before detecting a later 4,301-digit integer.
        for wrapped in [Envelope(member, limit), Envelope(limit, member)]:
            try:
                repr(wrapped.args)
            except ValueError as error:
                print(str(error))
            print(len(events))


asyncio.run(ordinary())
asyncio.run(failed_repr())
asyncio.run(mutations())
