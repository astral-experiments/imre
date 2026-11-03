from _imre_dataclasses import (
    is_defaultdict as _is_defaultdict,
    loaded_module as _loaded_module,
    make_type as _make_type, members as _members,
    native_iterator as _native_iterator, retarget_class_cell as _retarget_class_cell,
    slot_bases as _slot_bases,
    slotted_type as _slotted_type,
)
import types
from types import FunctionType, GenericAlias, MemberDescriptorType

__all__ = ["dataclass", "field", "Field", "fields", "is_dataclass", "make_dataclass", "asdict", "astuple", "replace",
           "MISSING", "KW_ONLY", "InitVar", "FrozenInstanceError"]

_FIELDS = "__dataclass_fields__"


class FrozenInstanceError(AttributeError):
    pass


class _KW_ONLY_TYPE:
    pass


KW_ONLY = _KW_ONLY_TYPE()


class InitVar:
    def __init__(self, type):
        self.type = type

    def __repr__(self):
        if isinstance(self.type, type):
            type_name = self.type.__name__
        else:
            type_name = repr(self.type)
        return f'dataclasses.InitVar[{type_name}]'

    @classmethod
    def __class_getitem__(cls, type):
        return InitVar(type)


class _FIELD_BASE:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return self.name


_FIELD = _FIELD_BASE('_FIELD')
_FIELD_CLASSVAR = _FIELD_BASE('_FIELD_CLASSVAR')
_FIELD_INITVAR = _FIELD_BASE('_FIELD_INITVAR')
_EMPTY_METADATA = types.MappingProxyType({})


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
                         types.MappingProxyType(metadata))
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


class _HAS_DEFAULT_FACTORY_CLASS:
    def __repr__(self):
        return '<factory>'


_HAS_DEFAULT_FACTORY = _HAS_DEFAULT_FACTORY_CLASS()


def _initializer(owner_class, data, positional, qualname, frozen, slots):
    from keyword import iskeyword
    from unicodedata import normalize

    cls = owner_class()
    receiver = '__dataclass_self__' if 'self' in cls.__dataclass_fields__ else 'self'
    # Join into exact strings so subclasses can't override validation or
    # formatting. The parser normalizes identifiers after tokenization.
    spellings = [''.join((f.name,)) for f in data.values()]
    symbols = {name: normalize('NFKC', name) for name in spellings}
    bound = {receiver}
    # Keep captured defaults and factories out of the class module's globals.
    constants_name = '__dataclass_values__'
    while constants_name in symbols.values():
        constants_name += '_'
    constants = [_HAS_DEFAULT_FACTORY, object.__setattr__]
    def capture(value):
        constants.append(value)
        return f'{constants_name}[{len(constants) - 1}]'

    parameters = {}
    body = []
    initvars = []
    for name, f in zip(spellings, data.values()):
        if not name.isidentifier() or iskeyword(name):
            raise SyntaxError(f'invalid field name {name!r}')
        symbol = symbols[name]
        if f.init:
            if symbol == '__debug__':
                raise SyntaxError('cannot assign to __debug__')
            if symbol in bound:
                raise SyntaxError(f'duplicate argument {symbol!r} in function definition')
            bound.add(symbol)
        parameter = name
        value = name
        if f.default_factory is not MISSING:
            factory = capture(f.default_factory)
            value = f'{factory}()'
            if f.init:
                marker = f'{constants_name}[0]'
                parameter += '=' + marker
                value = f'{value} if {name} is {marker} else {name}'
        elif f.default is not MISSING:
            if f.init:
                parameter += '=' + capture(f.default)
            elif slots:
                value = capture(f.default)
        if f.init:
            parameters[name] = parameter
        if f._field_type is _FIELD_INITVAR:
            initvars.append(name)
        elif f.init or f.default_factory is not MISSING or (slots and f.default is not MISSING):
            if frozen:
                body.append(f'{constants_name}[1]({receiver}, {name!r}, {value})')
            else:
                if symbol == '__debug__':
                    raise SyntaxError('cannot assign to __debug__')
                body.append(f'{receiver}.{name} = {value}')

    args = [receiver] + [parameters[f.name] for f in positional]
    kw_only = [parameters[f.name] for f in data.values() if f.init and f.kw_only]
    if kw_only:
        args += ['*'] + kw_only
    if hasattr(cls, '__post_init__'):
        body.append(f'{receiver}.__post_init__({", ".join(initvars)})')
    lines = [f'def build({constants_name}):', f' def __init__({", ".join(args)}):']
    lines.extend('  ' + line for line in (body or ['pass']))
    lines.append(' return __init__')
    owner = _loaded_module(cls.__module__)
    namespace = {}
    exec('\n'.join(lines), owner.__dict__ if owner is not None else {}, namespace)
    init = namespace['build'](tuple(constants))

    names = tuple(f.name for f in data.values() if f.init)
    def annotate(format, /):
        if format != 1:
            raise NotImplementedError(format)
        annotations = {}
        for base in reversed(owner_class().__mro__):
            annotations.update(getattr(base, "__annotations__", {}))
        result = {name: annotations[name] for name in names if name in annotations}
        result["return"] = None
        return result
    annotate.__generated_by_dataclasses__ = True
    annotate.__qualname__ = qualname + ".__annotate__"
    init.__annotate__ = annotate
    return init


def _annotation_marker(cls, annotation):
    # Like CPython's _is_type, resolve string markers in the defining module.
    owner = None
    if isinstance(annotation, str):
        import re
        match = re.match(r'^(?:\s*(\w+)\s*\.)?\s*(\w+)', annotation)
        module = _loaded_module(cls.__module__)
        if module is None or match is None:
            return None
        qualifier, name = match.groups()
        if qualifier is not None:
            owner = getattr(module, qualifier, None)
            if owner is None:
                return None
            module = owner
        annotation = getattr(module, name, None)
    if owner is None or owner is _loaded_module('dataclasses'):
        if annotation is KW_ONLY:
            return 'KW_ONLY'
        if annotation is InitVar or isinstance(annotation, InitVar):
            return 'InitVar'
    typing = _loaded_module('typing')
    if owner is not None and owner is not typing:
        return None
    classvar = getattr(typing, 'ClassVar', MISSING)
    if classvar is not MISSING and (annotation is classvar or typing.get_origin(annotation) is classvar):
        return 'ClassVar'
    return None


def _decorate(cls, params):
    own = _members(cls)
    slot_bases = _slot_bases(cls) if params.slots else ()
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
        default = getattr(cls, name, MISSING)
        if isinstance(default, MemberDescriptorType):
            default = MISSING
        f = default if isinstance(default, Field) else field(default=default)
        if marker in ('ClassVar', 'InitVar'):
            if f.default_factory is not MISSING:
                raise TypeError(f'field {name} cannot have a default factory')
            f._field_type = _FIELD_CLASSVAR if marker == 'ClassVar' else _FIELD_INITVAR
        else:
            if isinstance(f.default, (list, dict, set, bytearray)) or getattr(type(f.default), '__hash__', MISSING) is None:
                raise ValueError(f"mutable default {type(f.default)!r} for field {name} is not allowed: use default_factory")
            f._field_type = _FIELD
        if f._field_type is _FIELD_CLASSVAR:
            if f.kw_only is not MISSING:
                raise TypeError(f'field {name} is a ClassVar but specifies kw_only')
        elif f.kw_only is MISSING:
            f.kw_only = kw_only
        if isinstance(default, Field):
            if f.default is MISSING:
                delattr(cls, name)
            else:
                setattr(cls, name, f.default)
        f.name = name
        f.type = annotation
        data[name] = f
    for name, value in own.items():
        if isinstance(value, Field) and name not in annotations:
            raise TypeError(f'{name!r} is a field but has no type annotation')
    init_fields = {name: f for name, f in data.items() if f._field_type is not _FIELD_CLASSVAR}
    positional = [f for f in init_fields.values() if f.init and not f.kw_only]
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
        install('__init__', _initializer(lambda: cls, init_fields, positional, cls.__qualname__ + '.__init__', params.frozen, params.slots))
    if '__replace__' not in own:
        cls.__replace__ = _replace
    if params.match_args and '__match_args__' not in own:
        cls.__match_args__ = tuple(f.name for f in positional)
    real_fields = [f for f in data.values() if f._field_type is _FIELD]
    if params.repr:
        @_recursive_repr
        def __repr__(self):
            parts = [f.name + '=' + repr(getattr(self, f.name)) for f in real_fields if f.repr]
            return type(self).__qualname__ + '(' + ', '.join(parts) + ')'
        install('__repr__', __repr__)
    if params.eq:
        def __eq__(self, other):
            if self is other:
                return True
            if type(self) is not type(other):
                return NotImplemented
            for f in real_fields:
                if f.compare and not (getattr(self, f.name) == getattr(other, f.name)):
                    return False
            return True
        install('__eq__', __eq__)

    if params.order:
        names = tuple(f.name for f in real_fields if f.compare)

        def ordering(op):
            def compare(self, other):
                if other.__class__ is self.__class__:
                    left = [getattr(self, name) for name in names]
                    right = [getattr(other, name) for name in names]
                    return op(left, right)
                return NotImplemented
            return compare

        for name, op in (('__lt__', lambda a, b: a < b), ('__le__', lambda a, b: a <= b),
                         ('__gt__', lambda a, b: a > b), ('__ge__', lambda a, b: a >= b)):
            if name in own:
                raise TypeError(f'Cannot overwrite attribute {name} in class {cls.__name__} '
                                'Consider using functools.total_ordering')
            install(name, ordering(op))
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
                return hash_fields(tuple(getattr(self, f.name) for f in real_fields if (f.compare if f.hash is None else f.hash)))
            __hash__.__qualname__ = cls.__qualname__ + '.__hash__'
            cls.__hash__ = __hash__
        elif params.eq:
            cls.__hash__ = None
    frozen_owner = cls
    if params.frozen:
        field_names = tuple(f.name for f in real_fields)
        for name in ('__setattr__', '__delattr__'):
            if name in own:
                raise TypeError(f'Cannot overwrite attribute {name} in class {cls.__name__}')
        def __setattr__(self, name, value):
            if type(self) is frozen_owner or name in field_names:
                raise FrozenInstanceError(f'cannot assign to field {name!r}')
            super(frozen_owner, self).__setattr__(name, value)
        def __delattr__(self, name):
            if type(self) is frozen_owner or name in field_names:
                raise FrozenInstanceError(f'cannot delete field {name!r}')
            super(frozen_owner, self).__delattr__(name)
        install('__setattr__', __setattr__)
        install('__delattr__', __delattr__)
    if params.slots:
        # Generated methods may retain this frame. The original descriptor snapshot owns the old class.
        own.clear()
        cls, repaired_user_cell = _add_slots(cls, real_fields, params, slot_bases)
        # CPython stops after repairing the first __class__ cell. A user method
        # can have a separate cell from the one shared by generated methods.
        if not repaired_user_cell:
            frozen_owner = cls
    return cls


def _dataclass_getstate(self):
    return [getattr(self, f.name) for f in fields(self)]


def _dataclass_setstate(self, state):
    for f, value in zip(fields(self), state):
        object.__setattr__(self, f.name, value)


def _add_slots(cls, real_fields, params, slot_bases):
    namespace = _members(cls)
    if '__slots__' in namespace:
        raise TypeError(f'{cls.__name__} already specifies __slots__')
    inherited = set()
    for base, implicit in slot_bases:
        declared = base.__dict__.get('__slots__')
        if declared is None:
            inherited.update(implicit)
        elif isinstance(declared, str):
            inherited.add(declared)
        elif _native_iterator(declared) or hasattr(declared, '__next__'):
            raise TypeError(f"Slots of '{base.__name__}' cannot be determined")
        else:
            inherited.update(declared)
    slots = {f.name: f.doc for f in real_fields if f.name not in inherited}
    if params.weakref_slot and '__weakref__' not in inherited:
        slots['__weakref__'] = None
    namespace['__slots__'] = slots if any(doc is not None for doc in slots.values()) else tuple(slots)
    for f in real_fields:
        namespace.pop(f.name, None)
    namespace.pop('__dict__', None)
    namespace.pop('__weakref__', None)
    namespace['__qualname__'] = cls.__qualname__
    newcls = _slotted_type(cls, namespace)

    if params.frozen:
        if '__getstate__' not in namespace:
            newcls.__getstate__ = _dataclass_getstate
        if '__setstate__' not in namespace:
            newcls.__setstate__ = _dataclass_setstate

    for original in _members(newcls).values():
        member = original
        seen = {id(member): member}
        while not isinstance(member, type) and hasattr(member, '__wrapped__'):
            member = member.__wrapped__
            key = id(member)
            # Like inspect.unwrap, keep the objects alive and bound the chain.
            if key in seen or len(seen) >= 1000:
                raise ValueError(f'wrapper loop when unwrapping {original!r}')
            seen[key] = member
        if isinstance(member, property):
            if any(_retarget_class_cell(f, cls, newcls) for f in (member.fget, member.fset, member.fdel)):
                return newcls, True
        elif _retarget_class_cell(member, cls, newcls):
            return newcls, True
    return newcls, False


def dataclass(cls=None, /, *, init=True, repr=True, eq=True, order=False,
              unsafe_hash=False, frozen=False, match_args=True, kw_only=False,
              slots=False, weakref_slot=False):
    if order and not eq:
        raise ValueError('eq must be true if order is true')
    if weakref_slot and not slots:
        raise TypeError('weakref_slot is True but slots is False')
    params = _DataclassParams(init, repr, eq, order, unsafe_hash, frozen, match_args, kw_only, slots, weakref_slot)
    def wrap(cls):
        return _decorate(cls, params)
    return wrap if cls is None else wrap(cls)


def make_dataclass(cls_name, fields, *, bases=(), namespace=None, init=True,
                   repr=True, eq=True, order=False, unsafe_hash=False, frozen=False,
                   match_args=True, kw_only=False, slots=False, weakref_slot=False,
                   module=None, decorator=dataclass):
    """Create a class from field declarations and apply the requested decorator."""
    from keyword import iskeyword
    import sys

    annotations = {}
    defaults = {}
    implicit = False
    for item in fields:
        if isinstance(item, str):
            name = item
            annotation = None
            implicit = True
        elif len(item) == 2:
            name, annotation = item
        elif len(item) == 3:
            name, annotation, default = item
            defaults[name] = default
        else:
            raise TypeError(f'Invalid field: {item!r}')

        if not isinstance(name, str) or not name.isidentifier():
            raise TypeError(f'Field names must be valid identifiers: {name!r}')
        if iskeyword(name):
            raise TypeError(f'Field names must not be keywords: {name!r}')
        if name in annotations:
            raise TypeError(f'Field name duplicated: {name!r}')
        annotations[name] = annotation

    if implicit:
        raise NotImplementedError('implicit Any fields require deferred typing.Any resolution')
    if not isinstance(bases, tuple):
        raise TypeError('type.__new__() argument 2 must be tuple, not ' + type(bases).__name__)

    members = {}
    if namespace is not None:
        members.update(namespace)
    members.update(defaults)
    if '__slots__' in members or '__classcell__' in members:
        raise NotImplementedError('boxed make_dataclass requires an ordinary class namespace')
    if any(not isinstance(key, str) for key in members):
        raise NotImplementedError('boxed class namespaces require string keys')
    members.setdefault('__module__', 'types')
    members.setdefault('__doc__', None)
    members.setdefault('__qualname__', cls_name)
    if not isinstance(members['__qualname__'], str):
        raise TypeError('type __qualname__ must be a str')
    for key in ('__init_subclass__', '__class_getitem__'):
        value = members.get(key)
        if isinstance(value, FunctionType):
            members[key] = classmethod(value)
    if isinstance(members.get('__new__'), FunctionType):
        members['__new__'] = staticmethod(members['__new__'])
    cls = _make_type(cls_name, bases, members)

    def annotate(format):
        # The field types are already values. Rendering strings requires annotationlib.
        if format == 4:
            raise NotImplementedError(format)
        return annotations.copy()

    cls.__annotate__ = annotate
    if module is None:
        module = sys._getframemodulename(1) or '__main__'
    cls.__module__ = module
    return decorator(cls, init=init, repr=repr, eq=eq, order=order,
                     unsafe_hash=unsafe_hash, frozen=frozen, match_args=match_args,
                     kw_only=kw_only, slots=slots, weakref_slot=weakref_slot)


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
    obj_type = type(obj)
    if (issubclass(obj_type, dict) if named else isinstance(obj, dict)):
        if hasattr(obj_type, 'default_factory') or _is_defaultdict(obj):
            result = obj_type(obj.default_factory)
            for k, v in obj.items():
                result[_convert(k, factory, named)] = _convert(v, factory, named)
            return result
        return obj_type((_convert(k, factory, named), _convert(v, factory, named))
                        for k, v in obj.items())
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


def replace(obj, /, **changes):
    """Return a new object replacing specified fields with new values.

    This is especially useful for frozen classes.  Example usage::

      @dataclass(frozen=True)
      class C:
          x: int
          y: int

      c = C(1, 2)
      c1 = replace(c, x=3)
      assert c1.x == 3 and c1.y == 2
    """
    if not hasattr(type(obj), _FIELDS):
        raise TypeError("replace() should be called on dataclass instances")
    return _replace(obj, **changes)


def _replace(self, /, **changes):
    # We're going to mutate 'changes', but that's okay because it's a
    # new dict, even if called with 'replace(self, **my_changes)'.

    # It's an error to have init=False fields in 'changes'.
    # If a field is not in 'changes', read its value from the provided 'self'.

    for f in getattr(self, _FIELDS).values():
        # Only consider normal fields or InitVars.
        if f._field_type is not _FIELD and f._field_type is not _FIELD_INITVAR:
            continue

        if not f.init:
            # Error if this field is specified in changes.
            if f.name in changes:
                raise TypeError(f'field {f.name} is declared with '
                                f'init=False, it cannot be specified with '
                                f'replace()')
            continue

        if f.name not in changes:
            if f._field_type is _FIELD_INITVAR and f.default is MISSING:
                raise TypeError(f"InitVar {f.name!r} "
                                f'must be specified with replace()')
            changes[f.name] = getattr(self, f.name)

    # Create the new object, which calls __init__() and
    # __post_init__() (if defined), using all of the init fields we've
    # added and/or left in 'changes'.  If there are values supplied in
    # changes that aren't fields, this will correctly raise a
    # TypeError.
    return self.__class__(**changes)
