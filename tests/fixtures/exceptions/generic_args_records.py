import asyncio
from dataclasses import dataclass, field


@dataclass(slots=True)
class Ticket:
    value: int
    hidden: int = field(repr=False, compare=False)


class Envelope(Exception):
    def __init__(self, first, *rest):
        self.first = first
        super().__init__(first, *rest)


def carry(obj: object) -> object:
    return obj


def examine(error: BaseException, ticket: Ticket) -> None:
    args = error.args
    print(args is error.args)
    item = args[0]
    print(repr(item))
    print(isinstance(item, Ticket))
    print(isinstance(item, tuple))
    print(isinstance(item, dict))
    print(isinstance(item, ValueError))
    print(isinstance(item, (Ticket, ValueError)))
    print(item is ticket)
    if isinstance(item, Ticket):
        print(item.value)
    print(repr(args))


async def fail() -> None:
    raise ValueError("child")


async def observe_group() -> None:
    try:
        async with asyncio.TaskGroup() as tasks:
            tasks.create_task(fail())
    except ExceptionGroup as group:
        # A projected child has nominal exception identity, even beside a dataclass in a class tuple.
        member = group.args[1][0]
        print(isinstance(member, (ValueError, Ticket)))
        print(isinstance(member, dict))
        print(isinstance(member, tuple))
        try:
            raise group.exceptions[0]
        except ValueError as child:
            print(repr(child.args))


ticket = Ticket(42, 10**4300)
examine(Envelope(carry(ticket)), ticket)
examine(ValueError((42, 7)), ticket)

# The fixed-tuple argument owns its member through exception capture.
field_from_tuple = ValueError((ticket,)).args[0][0]
print(field_from_tuple is ticket)
print(isinstance(field_from_tuple, Ticket))
print(repr(field_from_tuple))
try:
    print(repr(Envelope(carry(Ticket(10**4300, 42))).args))
except ValueError as error:
    print(str(error))
asyncio.run(observe_group())
original_error = Envelope(carry(ticket))
ticket.value = 43
examine(original_error, ticket)
