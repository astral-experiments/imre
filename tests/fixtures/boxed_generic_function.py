import types
import typing


def current():
    events = []
    T = bytes

    def decorate(outer):
        events.append(outer)
        def capture(func):
            param, = func.__type_params__
            assert func.__annotations__ == {'value': param, 'return': param}
            events.append(param)
            return func
        return capture

    def factory():
        @decorate(T)
        def identity[T](value: T = T, /) -> T:
            return value
        return identity

    first, second = factory(), factory()
    t, = first.__type_params__
    assert isinstance(t, typing.TypeVar) and t.__name__ == 'T' and t.__infer_variance__
    assert t.evaluate_bound is t.evaluate_constraints is t.evaluate_default is None
    assert first.__defaults__ == (bytes,) and first() is bytes
    assert first(1) == 1 and first.__annotate__(1)['return'] is t
    assert not (t is second.__type_params__[0])
    assert events == [bytes, t, bytes, second.__type_params__[0]]

    class Owner:
        Alias = int

        def method[__U](self, x: __U, y: Alias) -> __U:
            return __U

    u, = Owner.method.__type_params__
    assert u.__name__ == '__U'
    assert Owner.method.__annotations__ == {'x': u, 'y': int, 'return': u}
    assert Owner().method(None, None) is u and not hasattr(Owner, '_Owner__U')

    def shadow[int](x: int) -> int:
        return x

    assert shadow('value') == 'value'
    assert shadow.__annotations__['x'] is shadow.__type_params__[0]
    clone = types.FunctionType(shadow.__code__, {}, "cloned")
    assert clone('restored') == 'restored'
    def nested[int]():
        def use[U](value: int) -> int:
            return value
        return use
    assert nested()('nested') == 'nested'

    class Cells:
        def parameter[__class__](self, arg: __class__):
            return __class__

        def owner[V](self):
            return __class__, V

        def via_super[V](self):
            return super().__thisclass__

        def global_class[V](self):
            global __class__
            return V, super()

    cls_t, = Cells.parameter.__type_params__
    assert Cells().parameter(0) is cls_t and Cells.parameter.__annotations__ == {'arg':cls_t}
    assert Cells.parameter.__closure__[0] is Cells.parameter.__annotate__.__closure__[0]
    assert Cells().owner() == (Cells, Cells.owner.__type_params__[0])
    assert Cells.owner.__code__.co_freevars == ('V', '__class__')
    method = Cells.via_super
    clone = types.FunctionType(method.__code__, {}, closure=method.__closure__)
    assert clone(Cells()) is Cells
    assert Cells.global_class.__code__.co_freevars == ('V',)
    try:
        Cells().global_class()
    except RuntimeError as error:
        assert str(error) == 'super(): __class__ cell not found'
    else:
        raise AssertionError('global declaration captured a class cell')
    def enclosing[T]():
        def middle():
            def annotated[U](T: T):
                return T
            return 'T' in locals(), annotated
        return middle

    present, annotated = enclosing()()
    assert present and annotated.__annotations__['T'] is enclosing.__type_params__[0]
    print('generic function scopes')


current()
