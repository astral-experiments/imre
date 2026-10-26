import asyncio
import weakref
from typing import Callable


class App:
    def __init__(self, debug: bool) -> None:
        self.debug = debug

    def __bool__(self) -> bool:
        return self.debug

    def __call__(self, value: int) -> int:
        return self.advance(value)

    def advance(self, value: int) -> int:
        return value + 1


class Provider:
    def __init__(self, app: App) -> None:
        self._app: App = weakref.proxy(app)


class Plain:
    def __init__(self, name: str) -> None:
        self.name = name


def orphaned() -> Provider:
    app = App(True)
    return Provider(app)


def retained_method() -> Callable[[int], int]:
    app = App(True)
    return weakref.proxy(app).advance


def drop_last(owners: list[App]) -> int:
    owners.clear()
    print("argument ran")
    return 41


def call_order() -> None:
    owners = [App(True)]
    proxy = weakref.proxy(owners[0])
    try:
        print(proxy(drop_last(owners)))
    except ReferenceError as error:
        print(str(error))


class AsyncApp:
    async def __call__(self, value: int) -> int:
        print("inside coroutine")
        return value + 1


async def async_call() -> None:
    app = AsyncApp()
    proxy = weakref.proxy(app)
    pending = proxy(3)
    print("created coroutine")
    print(await pending)


app = App(True)
print(app(3))
provider = Provider(app)
p = provider._app
print(p is weakref.proxy(app, None))
print(p is app)
print(p == app)
print(bool(p))
p.debug = False
print(app.debug)
print(bool(p))
print(p(value=41))
print(type(p) is weakref.CallableProxyType)
print(p.__class__ is App)
print(p.__class__.__name__)
print(isinstance(p, App))
print(type(p).__module__)

ordinary = Plain("plain")
q = weakref.proxy(ordinary)
print(q.name)
print(type(q) is weakref.ProxyType)
print(isinstance(q, weakref.CallableProxyType))
print(callable(q))

saved = orphaned()
dead = saved._app
print(type(dead) is weakref.CallableProxyType)
print(isinstance(dead, weakref.CallableProxyType))
print(dead is saved._app)
for action in (0, 1, 2, 3):
    try:
        if action == 0:
            print(dead.debug)
        elif action == 1:
            print(bool(dead))
        elif action == 2:
            print(dead(0))
        else:
            print(dead.__class__.__name__)
    except ReferenceError as error:
        print(str(error))
try:
    print(hash(dead))
except TypeError as error:
    print(str(error))
try:
    weakref.proxy(1)
except TypeError as error:
    print(str(error))
call_order()
advance = retained_method()
print(advance(9))
asyncio.run(async_call())
