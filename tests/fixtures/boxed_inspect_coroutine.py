import inspect
from functools import partial, update_wrapper
from inspect import cleandoc, iscoroutinefunction, markcoroutinefunction


assert inspect.cleandoc is cleandoc
assert cleandoc(doc="\n\tTitle\n\t  body\0 \r\n") == "Title\n  body\0 \r"
assert cleandoc("  first  \n  \u00a0second") == "first  \n\u00a0second"


async def view():
    raise AssertionError("inspection called the view")


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
