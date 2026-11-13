"""Attribute patching on live Python objects, including module namespaces."""

import builtins
import sys
from contextlib import ContextDecorator, ExitStack
from functools import wraps
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
_deleted = sentinel.DELETED


def _patch_owner(name):
    from pkgutil import resolve_name
    return resolve_name(name)


def _get_target(target):
    try:
        owner, attribute = target.rsplit('.', 1)
    except (AttributeError, TypeError, ValueError):
        raise TypeError(f"Need a valid target to patch. You supplied: {target!r}")
    return lambda: _patch_owner(owner), attribute


def _enter_patchings(stack, patchings, kwargs):
    extra = []
    for patching in patchings:
        replacement = stack.enter_context(patching)
        if patching.attribute_name is not None:
            kwargs.update(replacement)
        elif patching.new is DEFAULT:
            extra.append(replacement)
    return extra


def _decorate_patch_class(patcher, cls, *, inspect_all=False):
    for name in dir(cls):
        # patch.dict fetches every member before applying the test prefix.
        if not inspect_all and not name.startswith(patch.TEST_PREFIX):
            continue
        method = getattr(cls, name)
        if inspect_all and not name.startswith(patch.TEST_PREFIX):
            continue
        # Some boxed builtins are callable without exposing __call__.
        if hasattr(method, '__call__') or isinstance(method, type(len)):
            setattr(cls, name, patcher.copy()(method))
    return cls


class _patch:
    attribute_name = None
    _active_patches = _active_patches

    def __init__(self, getter, attribute, new, spec, create, spec_set, autospec,
                 new_callable, kwargs, *, unsafe=False):
        if new_callable is not None:
            if new is not DEFAULT:
                raise ValueError("Cannot use 'new' and 'new_callable' together")
            if autospec is not None:
                raise ValueError("Cannot use 'autospec' and 'new_callable' together")
        if not unsafe:
            _check_spec_arg_typos(kwargs)
        self.getter = getter
        self.attribute = attribute
        self.new = new
        self.create = create
        if _is_instance_mock(spec):
            raise InvalidSpecError(f"Cannot spec attr {attribute!r} as the spec "
                                   f"has already been mocked out. [spec={spec!r}]")
        if _is_instance_mock(spec_set):
            raise InvalidSpecError(f"Cannot spec attr {attribute!r} as the spec_set "
                                   f"target has already been mocked out. [spec_set={spec_set!r}]")
        self.spec = spec
        self.spec_set = spec_set
        self.autospec = autospec
        self.new_callable = new_callable
        self.mock_kwargs = kwargs
        self.additional_patchers = []
        self.is_started = False

    def _new_mock(self, original, spec, spec_set):
        if spec is True:
            spec = original
        if spec_set is True:
            spec_set = original if spec is None else spec
            spec = None
        effective_spec = spec if spec_set is None else spec_set
        if effective_spec is not None and original is DEFAULT:
            raise TypeError("Can't use 'spec' with create=True")

        factory = self.new_callable
        is_names = type(effective_spec) in (list, tuple)
        if factory is None:
            factory = MagicMock
            if spec is None and _is_async_obj(original):
                factory = AsyncMock
            elif effective_spec is not None:
                callable_spec = '__call__' in effective_spec if is_names else callable(effective_spec)
                if _is_async_obj(effective_spec):
                    factory = AsyncMock
                elif not callable_spec:
                    factory = NonCallableMagicMock

        options = {}
        if spec is not None:
            options['spec'] = spec
        if spec_set is not None:
            options['spec_set'] = spec_set
        if isinstance(factory, type) and issubclass(factory, NonCallableMock):
            options['name'] = self.attribute
        options.update(self.mock_kwargs)
        replacement = factory(**options)
        if effective_spec is not None and isinstance(original, type) and _is_instance_mock(replacement):
            # The class can be called, while the instances it stands for may not be.
            if not is_names and not _instance_callable(effective_spec):
                factory = NonCallableMagicMock
            options.pop('name')
            replacement.return_value = factory(_new_parent=replacement, _new_name='()', **options)
        return replacement

    def get_original(self):
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
        return original, local

    def __enter__(self):
        if self.is_started:
            raise RuntimeError("Patch is already started")
        self.target = self.getter()
        spec = None if self.spec is False else self.spec
        spec_set = None if self.spec_set is False else self.spec_set
        autospec = None if self.autospec is False else self.autospec
        if spec is not None and autospec is not None:
            raise TypeError("Can't specify spec and autospec")
        if (spec is not None or autospec is not None) and spec_set not in (True, None):
            raise TypeError("Can't provide explicit spec_set *and* spec or autospec")
        original, local = self.get_original()
        name = self.attribute
        replacement = self.new
        if autospec is not None:
            if replacement is not DEFAULT:
                raise TypeError("autospec creates the mock for you. Can't specify autospec and new.")
            if original is DEFAULT:
                raise TypeError("Can't use 'autospec' with create=True")
            if autospec is True:
                autospec = original
            if _is_instance_mock(self.target):
                raise InvalidSpecError(f"Cannot autospec attr {name!r} as the patch target has already "
                                       f"been mocked out. [target={self.target!r}, attr={autospec!r}]")
            if _is_instance_mock(autospec):
                target_name = getattr(self.target, '__name__', self.target)
                raise InvalidSpecError(f"Cannot autospec attr {name!r} from target {target_name!r} as it "
                                       f"has already been mocked out. [target={self.target!r}, attr={autospec!r}]")
            options = {'name': name}
            options.update(self.mock_kwargs)
            replacement = create_autospec(autospec, spec_set=bool(spec_set), **options)
        elif replacement is DEFAULT:
            replacement = self._new_mock(original, spec, spec_set)
        elif self.mock_kwargs:
            raise TypeError("Can't pass kwargs to a mock we aren't creating")
        self.temp_original = original
        self.is_local = local
        self._exit_stack = ExitStack()
        self.is_started = True
        try:
            setattr(self.target, name, replacement)
            if self.attribute_name is None:
                return replacement
            replacements = {}
            if self.new is DEFAULT:
                replacements[self.attribute_name] = replacement
            for patcher in self.additional_patchers:
                member = self._exit_stack.enter_context(patcher)
                if patcher.new is DEFAULT:
                    replacements.update(member)
            return replacements
        except BaseException:
            if not self.__exit__(*sys.exc_info()):
                raise

    def __exit__(self, *exc_info):
        if self.is_started:
            if self.is_local and self.temp_original is not DEFAULT:
                setattr(self.target, self.attribute, self.temp_original)
            else:
                delattr(self.target, self.attribute)
                # Deleting function metadata can reset the slot to a readable default.
                if not self.create and (
                    not hasattr(self.target, self.attribute)
                    or self.attribute in ('__doc__', '__module__', '__defaults__',
                                          '__annotations__', '__kwdefaults__')
                ):
                    setattr(self.target, self.attribute, self.temp_original)
            del self.target
            del self.temp_original
            del self.is_local
            self.is_started = False
            stack = self._exit_stack
            del self._exit_stack
            return stack.__exit__(*exc_info)

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

    def copy(self):
        patcher = _patch(self.getter, self.attribute, self.new, self.spec, self.create,
                         self.spec_set, self.autospec, self.new_callable, self.mock_kwargs)
        patcher.attribute_name = self.attribute_name
        patcher.additional_patchers = [child.copy() for child in self.additional_patchers]
        return patcher

    def __call__(self, func):
        if isinstance(func, type):
            return _decorate_patch_class(self, func)
        if hasattr(func, 'patchings'):
            func.patchings.append(self)
            return func

        from inspect import iscoroutinefunction
        if iscoroutinefunction(func):
            @wraps(func)
            async def patched(*args, **kwargs):
                with ExitStack() as stack:
                    extra = _enter_patchings(stack, patched.patchings, kwargs)
                    return await func(*args, *extra, **kwargs)
        else:
            @wraps(func)
            def patched(*args, **kwargs):
                with ExitStack() as stack:
                    extra = _enter_patchings(stack, patched.patchings, kwargs)
                    return func(*args, *extra, **kwargs)
        patched.patchings = [self]
        return patched


def _patch_attribute(self, target, new=DEFAULT, spec=None, create=False, spec_set=None,
                     autospec=None, new_callable=None, *, unsafe=False, **kwargs):
    getter, attribute = _get_target(target)
    return _patch(getter, attribute, new, spec, create, spec_set, autospec,
                  new_callable, kwargs, unsafe=unsafe)


def _patch_object(target, attribute, new=DEFAULT, spec=None, create=False, spec_set=None,
                  autospec=None, new_callable=None, *, unsafe=False, **kwargs):
    if type(target) is str:
        raise TypeError(f"{target!r} must be the actual object to be patched, not a str")
    return _patch(lambda: target, attribute, new, spec, create, spec_set, autospec,
                  new_callable, kwargs, unsafe=unsafe)


def _patch_multiple(target, spec=None, create=False, spec_set=None,
                    autospec=None, new_callable=None, **kwargs):
    if not kwargs:
        raise ValueError('Must supply at least one keyword argument with patch.multiple')
    getter = (lambda: _patch_owner(target)) if type(target) is str else (lambda: target)
    patchers = []
    for name, replacement in kwargs.items():
        patcher = _patch(getter, name, replacement, spec, create, spec_set, autospec,
                         new_callable, {})
        patcher.attribute_name = name
        patchers.append(patcher)
    first = patchers[0]
    first.additional_patchers = patchers[1:]
    return first


class _BoxedDictionaryPatch(_DictionaryPatch, ContextDecorator):
    _active_patches = _patch._active_patches
    start = _patch.start
    stop = _patch.stop

    def __init__(self, in_dict, values=(), clear=False, **kwargs):
        updates = dict(values)
        updates.update(kwargs)
        super().__init__(in_dict, updates, clear)

    def copy(self):
        return _BoxedDictionaryPatch(self.in_dict, self.values, self.clear)

    def __call__(self, func):
        if isinstance(func, type):
            return _decorate_patch_class(self, func, inspect_all=True)
        from inspect import iscoroutinefunction
        if iscoroutinefunction(func):
            @wraps(func)
            async def patched(*args, **kwargs):
                with self:
                    return await func(*args, **kwargs)
            return patched
        return ContextDecorator.__call__(self, func)

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

    __enter__ = _enter

    def __exit__(self, *exc_info):
        return self._exit()


_PatchFactory.__call__ = _patch_attribute
_PatchFactory.dict = staticmethod(_BoxedDictionaryPatch)
_PatchFactory.object = staticmethod(_patch_object)
_PatchFactory.multiple = staticmethod(_patch_multiple)


patch = _PatchFactory()
