# CPython 3.14.5 Lib/contextlib.py
from types import GenericAlias as _GenericAlias

# The typed compiler also loads the abstract base on its own.
AbstractContextManager.__class_getitem__ = classmethod(_GenericAlias)


class closing(AbstractContextManager):
    """Context to automatically close something at the end of a block.

    Code like this:

        with closing(<module>.open(<arguments>)) as f:
            <block>

    is equivalent to this:

        f = <module>.open(<arguments>)
        try:
            <block>
        finally:
            f.close()

    """
    def __init__(self, thing):
        self.thing = thing
    def __enter__(self):
        return self.thing
    def __exit__(self, *exc_info):
        self.thing.close()


class aclosing(AbstractAsyncContextManager):
    """Async context manager for safely finalizing an asynchronously cleaned-up
    resource such as an async generator, calling its ``aclose()`` method.

    Code like this:

        async with aclosing(<module>.fetch(<arguments>)) as agen:
            <block>

    is equivalent to this:

        agen = <module>.fetch(<arguments>)
        try:
            <block>
        finally:
            await agen.aclose()

    """
    def __init__(self, thing):
        self.thing = thing
    async def __aenter__(self):
        return self.thing
    async def __aexit__(self, *exc_info):
        await self.thing.aclose()
