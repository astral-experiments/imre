import collections
import inspect
import types
from functools import partial, partialmethod, update_wrapper
from inspect import CO_ASYNC_GENERATOR, CO_COROUTINE, CO_GENERATOR
from inspect import cleandoc, isawaitable, iscoroutinefunction, markcoroutinefunction


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

    deferred_partial = partialmethod(deferred)
    sync_partial = partialmethod(sync)

    async def __call__(self):
        pass


def make():
    def local():
        return None
    return local


views = Views()
assert iscoroutinefunction(Views.deferred_partial)
assert iscoroutinefunction(views.deferred_partial)
assert not iscoroutinefunction(Views.sync_partial)
assert not iscoroutinefunction(views.sync_partial)
before = views.sync
wrapped = partial(before)
for callback in (view, async_generator, factory, views, views.deferred, partial(views.deferred), wrapped, None):
    print(iscoroutinefunction(obj=callback))
print(markcoroutinefunction(func=before) is Views.sync)
assert not iscoroutinefunction(Views.sync_partial)
assert iscoroutinefunction(views.sync_partial)
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


# Runner.close() awaits the operation returned by aclose().
namespace = {}
exec("async def values(): yield 1", namespace)
agen = namespace["values"]()
step = agen.__anext__()
closer = agen.aclose()
coro = view()
assert isawaitable is inspect.isawaitable
assert isawaitable(coro) and isawaitable(closer)
assert not isawaitable(agen)
assert isawaitable(step)
step.close()
closer.close()
coro.close()


class Streams:
    def items(self):
        yield 1

    deferred = partialmethod(items)


assert inspect.isgeneratorfunction(Streams.deferred)
assert inspect.isgeneratorfunction(Streams().deferred)
assert inspect.isasyncgenfunction(partial(async_generator))
assert not inspect.isgeneratorfunction(async_generator)
assert inspect.isgenerator(Streams().items())
assert inspect.isasyncgen(agen) and type(agen) is types.AsyncGeneratorType
assert inspect.iscoroutine(coro) and not inspect.isgenerator(coro)
assert inspect.ispackage(collections) and not inspect.ispackage(types)
