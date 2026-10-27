from _weakref import (
    CallableProxyType, ProxyType, ReferenceType, _remove_dead_weakref,
    getweakrefcount, getweakrefs, proxy, ref,
)


class Item:
    pass


class ShadowedFields:
    __dict__ = "class attribute"


class CallableItem:
    def __call__(self, value):
        return self.value + value


class Slots:
    __slots__ = ()

class TupleChild(tuple):
    pass

class EqualKey:
    def __hash__(self):
        return -1

    def __eq__(self, other):
        return isinstance(other, EqualKey)


def orphan():
    local = CallableItem()
    return ref(local), proxy(local)


def current():
    observed = []

    def first(wr):
        observed.append(('first', wr() is None))

    def second(wr):
        observed.append(('second', wr() is None))

    receiver = CallableItem()
    receiver.value = 10
    handle = ref(receiver)
    forwarded = proxy(receiver)
    a = ref(receiver, first)
    b = ref(receiver, second)
    print(ref is ReferenceType, type(handle) is ReferenceType, type(forwarded) is CallableProxyType)
    print(ref(receiver) is handle, proxy(receiver) is forwarded, handle() is receiver)
    print(forwarded.__class__ is CallableItem, forwarded(4), forwarded.value)
    forwarded.value = 16
    print(receiver.value)
    forwarded.field = 1
    del forwarded.field
    print(hasattr(receiver, 'field'))
    refs = getweakrefs(receiver)
    print(getweakrefcount(receiver), [r is e for r, e in zip(refs, (handle, forwarded, b, a))])
    print(a.__callback__ is first, b.__callback__ is second)
    mapping = {'pending': b, 'alive': handle}
    _remove_dead_weakref(mapping, 'pending')
    print('pending' in mapping)
    del receiver
    print(observed, handle() is None, a.__callback__ is None)
    _remove_dead_weakref(mapping, 'pending')
    print('pending' in mapping)
    try:
        forwarded.value
    except ReferenceError as error:
        print(str(error))

    dead, dead_proxy = orphan()
    print(dead() is None)
    try:
        dead_proxy(3)
    except ReferenceError as error:
        print(str(error))

    target = Item()
    print(target.__weakref__ is None)
    observation = ref(target)
    print(target.__weakref__ is observation)
    try:
        target.__weakref__ = None
    except AttributeError:
        print('read only')

    other = Item()
    observer = proxy(other)
    methods = {"GET"}
    additions = {"OPTIONS"}
    methods_proxy = proxy(methods)
    print(methods_proxy | proxy(additions) == {"GET", "OPTIONS"}, methods_proxy is methods)
    methods_proxy |= proxy(additions)
    print(methods_proxy is methods, methods == {"GET", "OPTIONS"})
    print(type(observer) is ProxyType, not callable(observer), ref(other)() is other)
    class PlainError(ValueError):
        pass

    class WeakError(ValueError):
        __slots__ = ('__weakref__',)

    class ClosedError(ValueError):
        __slots__ = ()

    for error in (PlainError('plain'), WeakError('slots')):
        observation = ref(error)
        print(observation() is error)

    for value in (1, None, [], object(), Slots(), TupleChild(), ValueError(), ClosedError()):
        try:
            ref(value)
        except TypeError as error:
            print(str(error))
    for call, args in ((ref, ()), (proxy, ()), (getweakrefs, ()), (getweakrefcount, ()),
                       (ProxyType, ()), (handle, (1,)), (_remove_dead_weakref, ({},))):
        try:
            call(*args)
        except TypeError as error:
            print(str(error))

    temporary = ref(Item())
    print(temporary() is None)
    other.field = Item()
    field = ref(other.field)
    del other.field
    print(field() is None)
    values = [Item()]
    popped = ref(values[0])
    values.pop()
    print(popped() is None)

    def hold(item):
        def get():
            return item
        return get

    owner = hold(Item())
    observed = ref(owner())
    alias = owner
    del owner
    print(observed() is alias())
    alias = None
    print(observed() is None)

    owner = hold(Item())
    cell = owner.__closure__[0]
    previous = ref(cell.cell_contents)
    cell.cell_contents = Item()
    current_value = ref(owner())
    del owner
    print(previous() is None, cell.cell_contents is current_value())
    alias = cell
    del cell
    print(alias.cell_contents is current_value())
    del alias
    print(current_value() is None)

    first, second = EqualKey(), EqualKey()
    left, right = ref(first), ref(second)
    by_ref = {left: 'retained key'}
    print(left == right, left != right, by_ref[right], ref.__hash__(right))
    del first, second
    print(left() is None, left != right, left.__hash__())

    key = EqualKey()
    mapping = {key: Item(), 'discard': Item()}
    stored_key, stored_value = ref(key), ref(mapping[key])
    discarded = ref(mapping['discard'])
    del key
    kept = mapping.pop(EqualKey())
    mapping.pop('discard')
    print(stored_key() is None, stored_value() is kept, discarded() is None)

    holder = ShadowedFields()
    holder.keep = Item()
    holder.__dict__ = Item()
    kept, removed = ref(holder.keep), ref(holder.__dict__)
    del holder.__dict__
    print(holder.__dict__, kept() is holder.keep, removed() is None)

    holder = Item()
    holder.keep = Item()
    removed = ref(holder.keep)
    del holder.__dict__
    print(removed() is None)

    class Open:
        pass

    class Closed:
        __slots__ = ()

    names = ['__weakref__']

    class FromList:
        __slots__ = names

    class FromMapping:
        __slots__ = {'__weakref__': 'weak reference support'}

    Open.__slots__ = ()
    Closed.__slots__ = '__weakref__'
    names.clear()
    for target in (Open(), Closed(), FromList(), FromMapping()):
        try:
            observer = ref(target)
        except TypeError:
            print(False)
        else:
            print(observer() is target)

    class Rendered:
        def __str__(self):
            return 'weak target'

    rendered = Rendered()
    print(str(proxy(rendered)))
    try:
        str(dead_proxy)
    except ReferenceError as error:
        print(type(error).__name__)
