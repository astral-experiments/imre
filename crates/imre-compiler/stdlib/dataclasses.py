"""Dataclasses for native runtime-loaded Python classes.

Field construction, parameter metadata and the field queries follow CPython 3.14.5
Lib/dataclasses.py (PSF-2.0). Class methods are compiled closures on the original class.
"""
from _imre_dataclasses import loaded_module as _loaded_module, members as _members, metadata as _metadata
from types import GenericAlias

__all__ = ["dataclass", "field", "Field", "fields", "is_dataclass", "asdict", "astuple",
           "MISSING", "KW_ONLY", "InitVar", "FrozenInstanceError"]

_FIELDS = "__dataclass_fields__"


class FrozenInstanceError(AttributeError):
    pass


class _MISSING_TYPE:
    pass


MISSING = _MISSING_TYPE()


class _KW_ONLY_TYPE:
    pass


KW_ONLY = _KW_ONLY_TYPE()


class InitVar:
    def __init__(self, type):
        self.type = type

    def __class_getitem__(cls, type):
        return InitVar(type)
class _FIELD_BASE:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return self.name


_FIELD = _FIELD_BASE('_FIELD')
_EMPTY_METADATA = _metadata({})


def _recursive_repr(fn):
    running = {}
    def wrapper(self):
        for obj in running.values():
            if obj is self:
                return '...'
        slot = len(running)
        running[slot] = self
        try:
            return fn(self)
        finally:
            del running[slot]
    return wrapper


class Field:
    __slots__ = ('name',
                 'type',
                 'default',
                 'default_factory',
                 'repr',
                 'hash',
                 'init',
                 'compare',
                 'metadata',
                 'kw_only',
                 'doc',
                 '_field_type',  # Private: not to be used by user code.
                 )

    def __init__(self, default, default_factory, init, repr, hash, compare,
                 metadata, kw_only, doc):
        self.name = None
        self.type = None
        self.default = default
        self.default_factory = default_factory
        self.init = init
        self.repr = repr
        self.hash = hash
        self.compare = compare
        self.metadata = (_EMPTY_METADATA
                         if metadata is None else
                         _metadata(metadata))
        self.kw_only = kw_only
        self.doc = doc
        self._field_type = None

    @_recursive_repr
    def __repr__(self):
        return ('Field('
                f'name={self.name!r},'
                f'type={self.type!r},'
                f'default={self.default!r},'
                f'default_factory={self.default_factory!r},'
                f'init={self.init!r},'
                f'repr={self.repr!r},'
                f'hash={self.hash!r},'
                f'compare={self.compare!r},'
                f'metadata={self.metadata!r},'
                f'kw_only={self.kw_only!r},'
                f'doc={self.doc!r},'
                f'_field_type={self._field_type}'
                ')')

    # This is used to support the PEP 487 __set_name__ protocol in the
    # case where we're using a field that contains a descriptor as a
    # default value.  For details on __set_name__, see
    # https://peps.python.org/pep-0487/#implementation-details.
    #
    # Note that in _process_class, this Field object is overwritten
    # with the default value, so the end result is a descriptor that
    # had __set_name__ called on it at the right time.
    def __set_name__(self, owner, name):
        func = getattr(type(self.default), '__set_name__', None)
        if func:
            # There is a __set_name__ method on the descriptor, call
            # it.
            func(self.default, owner, name)

    __class_getitem__ = classmethod(GenericAlias)


def field(*, default=MISSING, default_factory=MISSING, init=True, repr=True,
          hash=None, compare=True, metadata=None, kw_only=MISSING, doc=None):
    """Return an object to identify dataclass fields.

    default is the default value of the field.  default_factory is a
    0-argument function called to initialize a field's value.  If init
    is true, the field will be a parameter to the class's __init__()
    function.  If repr is true, the field will be included in the
    object's repr().  If hash is true, the field will be included in the
    object's hash().  If compare is true, the field will be used in
    comparison functions.  metadata, if specified, must be a mapping
    which is stored but not otherwise examined by dataclass.  If kw_only
    is true, the field will become a keyword-only parameter to
    __init__().  doc is an optional docstring for this field.

    It is an error to specify both default and default_factory.
    """

    if default is not MISSING and default_factory is not MISSING:
        raise ValueError('cannot specify both default and default_factory')
    return Field(default, default_factory, init, repr, hash, compare,
                 metadata, kw_only, doc)


class _DataclassParams:
    __slots__ = ('init',
                 'repr',
                 'eq',
                 'order',
                 'unsafe_hash',
                 'frozen',
                 'match_args',
                 'kw_only',
                 'slots',
                 'weakref_slot',
                 )

    def __init__(self,
                 init, repr, eq, order, unsafe_hash, frozen,
                 match_args, kw_only, slots, weakref_slot):
        self.init = init
        self.repr = repr
        self.eq = eq
        self.order = order
        self.unsafe_hash = unsafe_hash
        self.frozen = frozen
        self.match_args = match_args
        self.kw_only = kw_only
        self.slots = slots
        self.weakref_slot = weakref_slot

    def __repr__(self):
        return ('_DataclassParams('
                f'init={self.init!r},'
                f'repr={self.repr!r},'
                f'eq={self.eq!r},'
                f'order={self.order!r},'
                f'unsafe_hash={self.unsafe_hash!r},'
                f'frozen={self.frozen!r},'
                f'match_args={self.match_args!r},'
                f'kw_only={self.kw_only!r},'
                f'slots={self.slots!r},'
                f'weakref_slot={self.weakref_slot!r}'
                ')')


def fields(class_or_instance):
    """Return a tuple describing the fields of this dataclass.

    Accepts a dataclass or an instance of one. Tuple elements are of
    type Field.
    """

    # Might it be worth caching this, per class?
    try:
        fields = getattr(class_or_instance, _FIELDS)
    except AttributeError:
        raise TypeError('must be called with a dataclass type or instance') from None

    # Exclude pseudo-fields.  Note that fields is sorted by insertion
    # order, so the order of the tuple is as the fields were defined.
    return tuple(f for f in fields.values() if f._field_type is _FIELD)


def is_dataclass(obj):
    """Returns True if obj is a dataclass or an instance of a
    dataclass."""
    cls = obj if isinstance(obj, type) else type(obj)
    return hasattr(cls, _FIELDS)


def _missing(names, qualname, kind):
    if not names:
        return
    quoted = [repr(n) for n in names]
    if len(names) == 1:
        joined = quoted[0]
    elif len(names) == 2:
        joined = ' and '.join(quoted)
    else:
        joined = ', '.join(quoted[:-1]) + ', and ' + quoted[-1]
    plural = '' if len(names) == 1 else 's'
    raise TypeError(f'{qualname}() missing {len(names)} required {kind} argument{plural}: {joined}')


def _initializer(cls, data, positional, qualname, frozen):
    required = [f for f in positional if f.default is MISSING and f.default_factory is MISSING]
    has_post_init = hasattr(cls, '__post_init__')
    def __init__(self, /, *args, **kwargs):
        values = {}
        for f, value in zip(positional, args):
            values[f.name] = value
        for name, value in kwargs.items():
            if name in values or (name == 'self' and 'self' not in data):
                raise TypeError(f"{qualname}() got multiple values for argument {name!r}")
            if name not in data or not data[name].init:
                raise TypeError(f"{qualname}() got an unexpected keyword argument {name!r}")
            values[name] = value
        if len(args) > len(positional):
            # CPython counts the bound receiver when reporting positional arity.
            lo = len(required) + 1
            hi = len(positional) + 1
            count = str(hi) if lo == hi else f'from {lo} to {hi}'
            plural = '' if hi == 1 else 's'
            supplied = str(len(args) + 1)
            kw_count = len([name for name in kwargs if data[name].kw_only])
            if kw_count:
                kw_plural = '' if kw_count == 1 else 's'
                supplied += f' positional arguments (and {kw_count} keyword-only argument{kw_plural})'
            raise TypeError(f'{qualname}() takes {count} positional argument{plural} but {supplied} were given')
        _missing([f.name for f in required if f.name not in values], qualname, 'positional')
        _missing([f.name for f in data.values() if f.kw_only and f.init and f.name not in values and f.default is MISSING and f.default_factory is MISSING], qualname, 'keyword-only')
        for f in data.values():
            if f.name in values:
                value = values[f.name]
            elif f.default_factory is not MISSING:
                value = f.default_factory()
            elif f.init:
                value = f.default
            else:
                continue
            if frozen:
                object.__setattr__(self, f.name, value)
            else:
                setattr(self, f.name, value)
        if has_post_init:
            self.__post_init__()
    return __init__


def _annotation_marker(cls, annotation):
    # Like CPython's _is_type, resolve string markers in the defining module.
    if isinstance(annotation, str):
        name = annotation.partition('[')[0].strip()
        module = _loaded_module(cls.__module__)
        if module is None:
            return None
        owner, dot, attr = name.partition('.')
        if dot:
            owner = getattr(module, owner.strip(), None)
            if owner is None:
                return None
            if owner is _loaded_module('typing') and attr.strip() == 'ClassVar':
                return 'ClassVar'
            annotation = getattr(owner, attr.strip(), None)
        else:
            annotation = getattr(module, name, None)
    if annotation is KW_ONLY:
        return 'KW_ONLY'
    if annotation is InitVar or isinstance(annotation, InitVar):
        return 'InitVar'
    typing = _loaded_module('typing')
    classvar = getattr(typing, 'ClassVar', MISSING)
    if classvar is not MISSING and (annotation is classvar or getattr(annotation, '__origin__', None) is classvar):
        return 'ClassVar'
    return None


def _decorate(cls, params):
    own = _members(cls)
    data = {}
    frozen_bases = []
    for base in cls.__mro__[-1:0:-1]:
        inherited = getattr(base, '__dataclass_fields__', None)
        if inherited is not None:
            data.update(inherited)
            frozen_bases.append(base.__dataclass_params__.frozen)
    if params.frozen and False in frozen_bases:
        raise TypeError('cannot inherit frozen dataclass from a non-frozen one')
    if not params.frozen and True in frozen_bases:
        raise TypeError('cannot inherit non-frozen dataclass from a frozen one')
    annotations = cls.__annotations__
    kw_only = params.kw_only
    seen_kw_only = False
    for name, annotation in annotations.items():
        marker = _annotation_marker(cls, annotation)
        if marker == 'KW_ONLY':
            if seen_kw_only:
                raise TypeError(f'{name!r} is KW_ONLY, but KW_ONLY has already been specified')
            seen_kw_only = kw_only = True
            continue
        if marker is not None:
            raise NotImplementedError(f'boxed dataclass {marker} fields')
        default = getattr(cls, name, MISSING)
        f = default if isinstance(default, Field) else field(default=default)
        if isinstance(default, Field):
            if f.default is MISSING:
                delattr(cls, name)
            else:
                setattr(cls, name, f.default)
        if isinstance(f.default, (list, dict, set, bytearray)) or getattr(type(f.default), '__hash__', MISSING) is None:
            raise ValueError(f"mutable default {type(f.default)!r} for field {name} is not allowed: use default_factory")
        f.name = name
        f.type = annotation
        f._field_type = _FIELD
        if f.kw_only is MISSING:
            f.kw_only = kw_only
        data[name] = f
    for name, value in own.items():
        if isinstance(value, Field) and name not in annotations:
            raise TypeError(f'{name!r} is a field but has no type annotation')
    positional = [f for f in data.values() if f.init and not f.kw_only]
    cls.__dataclass_fields__ = data
    cls.__dataclass_params__ = params

    def install(name, fn):
        if name not in own:
            fn.__name__ = name
            fn.__qualname__ = cls.__qualname__ + '.' + name
            setattr(cls, name, fn)

    if params.init:
        last_default = None
        for f in positional:
            if f.default is not MISSING or f.default_factory is not MISSING:
                last_default = f.name
            elif last_default is not None:
                raise TypeError(f'non-default argument {f.name!r} follows default argument {last_default!r}')
        install('__init__', _initializer(cls, data, positional, cls.__qualname__ + '.__init__', params.frozen))
    if params.match_args and '__match_args__' not in own:
        cls.__match_args__ = tuple(f.name for f in positional)
    if params.repr:
        @_recursive_repr
        def __repr__(self):
            parts = [f.name + '=' + repr(getattr(self, f.name)) for f in data.values() if f.repr]
            return type(self).__qualname__ + '(' + ', '.join(parts) + ')'
        install('__repr__', __repr__)
    if params.eq:
        def __eq__(self, other):
            if self is other:
                return True
            if type(self) is not type(other):
                return NotImplemented
            for f in data.values():
                if f.compare and not (getattr(self, f.name) == getattr(other, f.name)):
                    return False
            return True
        install('__eq__', __eq__)
    explicit_hash = '__hash__' in own and not (own['__hash__'] is None and '__eq__' in own)
    if params.unsafe_hash and explicit_hash:
        raise TypeError(f'Cannot overwrite attribute __hash__ in class {cls.__name__}')
    if not explicit_hash:
        if params.unsafe_hash or (params.eq and params.frozen):
            def __hash__(self):
                try:
                    hash_fields = hash
                except NameError:
                    raise NotImplementedError('boxed dataclass hashing requires the hash builtin') from None
                return hash_fields(tuple(getattr(self, f.name) for f in data.values() if (f.compare if f.hash is None else f.hash)))
            install('__hash__', __hash__)
        elif params.eq:
            cls.__hash__ = None
    if params.frozen:
        for name in ('__setattr__', '__delattr__'):
            if name in own:
                raise TypeError(f'Cannot overwrite attribute {name} in class {cls.__name__}')
        def __setattr__(self, name, value):
            if type(self) is cls or name in data:
                raise FrozenInstanceError(f'cannot assign to field {name!r}')
            super(cls, self).__setattr__(name, value)
        def __delattr__(self, name):
            if type(self) is cls or name in data:
                raise FrozenInstanceError(f'cannot delete field {name!r}')
            super(cls, self).__delattr__(name)
        install('__setattr__', __setattr__)
        install('__delattr__', __delattr__)
    return cls


def dataclass(cls=None, /, *, init=True, repr=True, eq=True, order=False,
              unsafe_hash=False, frozen=False, match_args=True, kw_only=False,
              slots=False, weakref_slot=False):
    if order and not eq:
        raise ValueError('eq must be true if order is true')
    if weakref_slot and not slots:
        raise TypeError('weakref_slot is True but slots is False')
    if order or slots:
        raise NotImplementedError('boxed dataclass ordering and slots')
    params = _DataclassParams(init, repr, eq, order, unsafe_hash, frozen, match_args, kw_only, slots, weakref_slot)
    def wrap(cls):
        return _decorate(cls, params)
    return wrap if cls is None else wrap(cls)


def _convert(obj, factory, named):
    if hasattr(type(obj), '__dataclass_fields__'):
        if named:
            return factory([(f.name, _convert(getattr(obj, f.name), factory, named)) for f in fields(obj)])
        return factory([_convert(getattr(obj, f.name), factory, named) for f in fields(obj)])
    if isinstance(obj, tuple) and hasattr(obj, '_fields'):
        return type(obj)(*[_convert(x, factory, named) for x in obj])
    if isinstance(obj, (list, tuple)):
        return type(obj)([_convert(x, factory, named) for x in obj])
    if type(obj) is dict:
        return {_convert(k, factory, named): _convert(v, factory, named) for k, v in obj.items()}
    if obj is None or type(obj) in (bool, int, float, str, bytes):
        return obj
    from copy import deepcopy
    return deepcopy(obj)


def asdict(obj, *, dict_factory=dict):
    if not hasattr(type(obj), '__dataclass_fields__'):
        raise TypeError('asdict() should be called on dataclass instances')
    return _convert(obj, dict_factory, True)


def astuple(obj, *, tuple_factory=tuple):
    if not hasattr(type(obj), '__dataclass_fields__'):
        raise TypeError('astuple() should be called on dataclass instances')
    return _convert(obj, tuple_factory, False)
