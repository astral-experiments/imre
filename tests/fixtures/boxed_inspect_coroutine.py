import inspect
from functools import partial, partialmethod, update_wrapper
from inspect import CO_ASYNC_GENERATOR, CO_COROUTINE, CO_GENERATOR
from inspect import cleandoc, isasyncgenfunction, iscoroutinefunction, markcoroutinefunction


assert inspect.cleandoc is cleandoc
assert cleandoc(doc="\n\tTitle\n\t  body\0 \r\n") == "Title\n  body\0 \r"
assert cleandoc("  first  \n  \u00a0second") == "first  \n\u00a0second"


async def view():
    raise AssertionError("inspection called the view")


print(CO_GENERATOR, CO_COROUTINE, CO_ASYNC_GENERATOR)
print(view.__code__.co_flags & CO_COROUTINE)


async def async_generator():
    yield 1


def factory():
    return view()


@markcoroutinefunction
def marked_factory():
    return view()


class Views:
    def sync(self):
        pass

    async def deferred(self):
        pass

    async def __call__(self):
        pass


def make():
    def local():
        return None
    return local


views = Views()
before = views.sync
wrapped = partial(before)
for callback in (view, async_generator, factory, views, views.deferred, partial(views.deferred), wrapped, None):
    print(iscoroutinefunction(obj=callback))
print(markcoroutinefunction(func=before) is Views.sync)
print(iscoroutinefunction(before), iscoroutinefunction(views.sync), iscoroutinefunction(wrapped))
factory._is_coroutine_marker = True
print(iscoroutinefunction(factory))
factory._is_coroutine_marker = before._is_coroutine_marker
print(iscoroutinefunction(factory))
factory._is_coroutine_marker = False
outer = partial(factory)
print(markcoroutinefunction(outer) is outer, iscoroutinefunction(outer))
print(markcoroutinefunction(views) is views, iscoroutinefunction(views))
update_wrapper(factory, view)
print(iscoroutinefunction(factory))
update_wrapper(factory, marked_factory)
print(iscoroutinefunction(factory))
one, two = make(), make()
print(markcoroutinefunction(one) is one, iscoroutinefunction(one), iscoroutinefunction(two))
for attempt in (lambda: iscoroutinefunction(), lambda: iscoroutinefunction(view, obj=view),
                lambda: iscoroutinefunction(view, 1), lambda: iscoroutinefunction(other=view)):
    try:
        attempt()
    except TypeError as error:
        print(str(error))

# The coroutine flag belongs to the code object and the marker to the callable.
def plain():
    return None


saved = plain.__code__
alias = partial(plain)
plain.__code__ = view.__code__
print(iscoroutinefunction(plain), iscoroutinefunction(alias))
plain.__code__ = saved
print(iscoroutinefunction(plain), iscoroutinefunction(alias))

# Inspect the executable code flag without calling the function.
def sync_generator():
    yield 1


async def nested_only():
    def inner():
        yield 1


class Sources:
    async def events(self, first):
        yield first

    first = partialmethod(events, 1)


class FunctionLike:
    __name__ = "events"
    __code__ = Sources.events.__code__
    __defaults__ = None
    __kwdefaults__ = None

    def __call__(self):
        raise AssertionError("inspection called a function-like object")


assert inspect.isasyncgenfunction is isasyncgenfunction
flags = CO_ASYNC_GENERATOR | CO_COROUTINE | CO_GENERATOR
assert async_generator.__code__.co_flags & flags == CO_ASYNC_GENERATOR
assert nested_only.__code__.co_flags & flags == CO_COROUTINE
assert sync_generator.__code__.co_flags & flags == CO_GENERATOR
sources = Sources()
for obj in (async_generator, Sources.events, sources.events, partial(partial(sources.events), 1),
            Sources.first, sources.first, vars(Sources)["first"], FunctionLike()):
    assert isasyncgenfunction(obj=obj)
for obj in (view, sync_generator, nested_only, marked_factory, factory, FunctionLike, 1):
    assert not isasyncgenfunction(obj)
duck = FunctionLike()
duck.__defaults__ = []
assert not isasyncgenfunction(duck)

saved = plain.__code__
plain.__code__ = async_generator.__code__
assert isasyncgenfunction(plain) and isasyncgenfunction(alias)
assert not iscoroutinefunction(plain)
plain.__code__ = saved
assert not isasyncgenfunction(plain) and not isasyncgenfunction(alias)
print("async generator function flags")
