"""Attribute patching on live Python objects, including module namespaces."""

import builtins
from contextlib import ContextDecorator
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


def _get_target(target):
    try:
        owner, attribute = target.rsplit('.', 1)
    except (AttributeError, TypeError, ValueError):
        raise TypeError(f"Need a valid target to patch. You supplied: {target!r}")
    return lambda: _patch_owner(owner), attribute


class _patch:
    _active_patches = []

    def __init__(self, getter, attribute, new, create, *, new_callable=None, **kwargs):
        if new_callable is not None:
            if new is not DEFAULT:
                raise ValueError("Cannot use 'new' and 'new_callable' together")
            if kwargs.get('autospec') is not None:
                raise ValueError("Cannot use 'autospec' and 'new_callable' together")
        # These are patch options, not arguments to the replacement factory.
        for option in ('spec', 'spec_set', 'autospec', 'unsafe'):
            if option in kwargs:
                raise TypeError(f"unsupported patch option: {option!r}")
        self.getter = getter
        self.attribute = attribute
        self.new = new
        self.create = create
        self.new_callable = new_callable
        self.mock_kwargs = kwargs
        self.is_started = False

    def __enter__(self):
        if self.is_started:
            raise RuntimeError("Patch is already started")
        target = self.getter()
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
        replacement = self.new
        if replacement is DEFAULT:
            factory = MagicMock if self.new_callable is None else self.new_callable
            kwargs = {}
            if isinstance(factory, type) and issubclass(factory, NonCallableMock):
                kwargs['name'] = name
            kwargs.update(self.mock_kwargs)
            replacement = factory(**kwargs)
        elif self.mock_kwargs:
            raise TypeError("Can't pass kwargs to a mock we aren't creating")
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
            return False

    def start(self):
        replacement = self.__enter__()
        self._active_patches.append(self)
        return replacement

    def stop(self):
        try:
            self._active_patches.remove(self)
        except ValueError:
            return None
        return self.__exit__(None, None, None)

    def __call__(self, func):
        if isinstance(func, type):
            for name in dir(func):
                if name.startswith(patch.TEST_PREFIX):
                    method = getattr(func, name)
                    # Some boxed builtins are callable without exposing __call__.
                    if hasattr(method, '__call__') or isinstance(method, type(len)):
                        patcher = _patch(self.getter, self.attribute, self.new, self.create,
                                         new_callable=self.new_callable, **self.mock_kwargs)
                        setattr(func, name, patcher(method))
            return func
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


def _patch_attribute(self, target, new=DEFAULT, *, create=False, **kwargs):
    getter, attribute = _get_target(target)
    return _patch(getter, attribute, new, create, **kwargs)


def _patch_object(target, attribute, new=DEFAULT, *, create=False, **kwargs):
    if type(target) is str:
        raise TypeError(f"{target!r} must be the actual object to be patched, not a str")
    return _patch(lambda: target, attribute, new, create, **kwargs)


class _BoxedDictionaryPatch(_DictionaryPatch, ContextDecorator):
    def __init__(self, in_dict, values=(), clear=False, **kwargs):
        updates = dict(values)
        updates.update(kwargs)
        super().__init__(in_dict, updates, clear)

    def _enter(self):
        if isinstance(self.in_dict, str):
            self.in_dict = _patch_owner(self.in_dict)
        mapping = self.in_dict
        try:
            self._original = mapping.copy()
        except AttributeError:
            self._original = {key: mapping[key] for key in mapping}
        self._change(self.values, self.clear)
        return mapping

    def _change(self, values, clear):
        mapping = self.in_dict
        if clear:
            try:
                mapping.clear()
            except AttributeError:
                for key in list(mapping):
                    del mapping[key]
        try:
            mapping.update(values)
        except AttributeError:
            for key in values:
                mapping[key] = values[key]

    def _exit(self):
        if self._original is not None:
            self._change(self._original, True)
        return False

    __enter__ = _enter

    def __exit__(self, *exc_info):
        return self._exit()


_PatchFactory.__call__ = _patch_attribute
_PatchFactory.dict = staticmethod(_BoxedDictionaryPatch)
_PatchFactory.object = staticmethod(_patch_object)


patch = _PatchFactory()
