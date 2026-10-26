import asyncio
from functools import partial
from inspect import iscoroutinefunction, markcoroutinefunction
from typing import Callable


def ordinary(value: int) -> int:
    return value


def marked(value: int) -> int:
    return value


@markcoroutinefunction
def decorated(value: int) -> int:
    return value


async def asynchronous(value: int) -> int:
    raise AssertionError("inspection entered the async body")


class Views:
    def normal(self, value: int) -> int:
        return value

    async def deferred(self, value: int) -> int:
        return value + 2


def inspect_one(view: Callable[[int], int]) -> bool:
    return iscoroutinefunction(view)


def inspect_zero(view: Callable[[], int]) -> bool:
    return iscoroutinefunction(view)


def bind(view: Callable[[int], int]) -> bool:
    return inspect_zero(partial(view, 4))


views = Views()
alias = marked
bound = views.normal
before = partial(marked, 3)
print(iscoroutinefunction(asynchronous))
print(iscoroutinefunction(views.deferred))
print(iscoroutinefunction(decorated))
print(iscoroutinefunction(partial(decorated, 1)))
deferred = partial(views.deferred)
print(iscoroutinefunction(deferred))
print(iscoroutinefunction(partial(partial(asynchronous))))
print(asyncio.run(deferred(40)))
print(iscoroutinefunction(obj=ordinary))
print(iscoroutinefunction(bound))
print(iscoroutinefunction(3))
print(inspect_one(alias))
print(inspect_zero(before))
print(bind(alias))
print(markcoroutinefunction(func=marked) is alias)
print(inspect_one(alias))
print(inspect_zero(before))
print(bind(alias))
print(inspect_one(ordinary))
print(inspect_zero(partial(ordinary, 3)))
print(bind(ordinary))
