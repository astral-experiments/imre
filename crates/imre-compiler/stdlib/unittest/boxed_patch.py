"""Attribute patching on live Python objects, including module namespaces."""

import builtins
from functools import wraps
from importlib import import_module
from types import ModuleType


_builtins = {name for name in dir(builtins) if not name.startswith('_')}


# CPython 3.14.5 Lib/unittest/mock.py
class _SentinelObject(object):
    "A unique, named, sentinel object."
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return 'sentinel.%s' % self.name

    def __reduce__(self):
        return 'sentinel.%s' % self.name


class _Sentinel(object):
    """Access attributes to return a named object, usable as a sentinel."""
    def __init__(self):
        self._sentinels = {}

    def __getattr__(self, name):
        if name == '__bases__':
            # Without this help(unittest.mock) raises an exception
            raise AttributeError
        return self._sentinels.setdefault(name, _SentinelObject(name))

    def __reduce__(self):
        return 'sentinel'


sentinel = _Sentinel()

DEFAULT = sentinel.DEFAULT


def _patch_owner(name):
    # Import the longest module prefix, then look up the remaining attributes.
    parts = name.split('.')
    prefix = parts.pop(0)
    owner = import_module(prefix)
    while parts:
        candidate = prefix + '.' + parts[0]
        try:
            owner = import_module(candidate)
        except ImportError:
            break
        prefix = candidate
        parts.pop(0)
    for part in parts:
        owner = getattr(owner, part)
    return owner


class _patch:
    def __init__(self, target, new, create):
        try:
            self.owner_name, self.attribute = target.rsplit('.', 1)
        except (AttributeError, TypeError, ValueError):
            raise TypeError(f"Need a valid target to patch. You supplied: {target!r}")
        self.new = new
        self.create = create
        self.is_started = False

    def __enter__(self):
        if self.is_started:
            raise RuntimeError("Patch is already started")
        target = _patch_owner(self.owner_name)
        name = self.attribute
        try:
            original = target.__dict__[name]
            local = True
        except (AttributeError, KeyError):
            original = getattr(target, name, DEFAULT)
            local = False
        if isinstance(target, ModuleType) and name in _builtins:
            self.create = True
        if original is DEFAULT and not self.create:
            raise AttributeError(f"{target} does not have the attribute {name!r}")
        replacement = MagicMock(name=name) if self.new is DEFAULT else self.new
        setattr(target, name, replacement)
        self.target = target
        self.original = original
        self.local = local
        self.is_started = True
        return replacement

    def __exit__(self, *exc_info):
        if self.is_started:
            if self.local:
                setattr(self.target, self.attribute, self.original)
            else:
                delattr(self.target, self.attribute)
                # Deleting function metadata can reset the slot to a readable default.
                if not self.create and (
                    not hasattr(self.target, self.attribute)
                    or self.attribute in ('__doc__', '__module__', '__defaults__',
                                          '__annotations__', '__kwdefaults__')
                ):
                    setattr(self.target, self.attribute, self.original)
            del self.target
            del self.original
            self.is_started = False

    def __call__(self, func):
        if hasattr(func, 'patchings'):
            func.patchings.append(self)
            return func

        @wraps(func)
        def patched(*args, **kwargs):
            from contextlib import ExitStack
            with ExitStack() as stack:
                extra = []
                for patching in patched.patchings:
                    replacement = stack.enter_context(patching)
                    if patching.new is DEFAULT:
                        extra.append(replacement)
                return func(*args, *extra, **kwargs)
        patched.patchings = [self]
        return patched


def _patch_attribute(self, target, new=DEFAULT, *, create=False):
    return _patch(target, new, create)


_PatchFactory.__call__ = _patch_attribute
