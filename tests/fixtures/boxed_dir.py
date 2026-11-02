class Parent:
    inherited = 1

    @property
    def untouched(self):
        raise AssertionError('dir evaluated a property')


class Child(Parent):
    local = 2


class Override:
    def __dir__(self):
        return ('z', 'a', 'a')


class ExceptionDirectory(ValueError):
    def __dir__(self):
        return ('z', 'a', 'a')


class Broken:
    def __dir__(self):
        return 7


class DictParent:
    @property
    def __dict__(self):
        return {'from_mapping': 1, 'shared': 2}


class DictChild(DictParent):
    shared = 3


class Exposed:
    visible = 1


class Masked:
    hidden = 2

    @property
    def __class__(self):
        return Exposed


class Slots:
    __slots__ = ('__private', 'slot')


class WrongArity:
    def __dir__(self, required):
        return ()


class Mixed:
    def __dir__(self):
        return ['a', 1]


def tuple_directories():
    from collections import namedtuple

    extra = ['__add__', '__class_getitem__', '__contains__', '__getitem__', '__getnewargs__',
             '__iter__', '__len__', '__mul__', '__rmul__', 'count', 'index']
    assert dir(()) == dir(tuple) == sorted(dir(object) + extra)
    point = namedtuple('Point', 'x y')
    value = point(1, 2)
    assert point.__name__ == 'Point' and '__name__' not in dir(value)
    assert '__weakref__' not in dir(value) and '__dict__' not in dir(value)
    assert 'x' in dir(value)
    del point.x
    assert 'x' not in dir(value) and value[0] == 1

    class Child(point):
        pass

    child = Child(3, 4)
    child.local = True
    assert 'local' in dir(child) and 'x' not in dir(child)
    assert '__dict__' in dir(child) and '__weakref__' not in dir(child)

    class Slotted(tuple):
        __slots__ = ()
        __weakref__ = 'visible class binding'

    Slotted.__slots__ = ('phantom',)
    assert 'phantom' not in dir(Slotted()) and '__weakref__' in dir(Slotted())
    del Slotted.__slots__
    del Slotted.__weakref__
    assert '__weakref__' not in dir(Slotted()) and '__dict__' not in dir(Slotted())

    class Reported(tuple):
        @property
        def __dict__(self):
            return {'stored': 1}

        @property
        def __class__(self):
            return Exposed

    names = dir(Reported())
    assert 'visible' in names and 'stored' in names and 'count' not in names

    class Meta(type):
        @property
        def __dict__(cls):
            return {'announced': True}

        @property
        def __bases__(cls):
            return ()

    class Published(tuple, metaclass=Meta):
        hidden = 1

    assert dir(Published) == dir(Published()) == ['announced']


def current():
    import _io
    tuple_directories()
    operation = dir
    names = operation(len)
    print(names)
    for value in (_io.open, [].append, type(len)):
        result = operation(value)
        assert result == names and result is not names
    assert '__self__' not in operation(Child)
    value = Child()
    value.stored = 3
    print(operation(object()), operation(object) == operation(object()))
    print(operation(Child), operation(value))
    value.__dir__ = lambda: ['shadow']
    print('shadow' in operation(value), 'stored' in operation(value))
    mapping = DictChild()
    mapping.hidden = 4
    print('from_mapping' in operation(mapping), 'hidden' in operation(mapping), [name for name in operation(mapping) if name == 'shared'])
    masked = Masked()
    print('visible' in operation(masked), 'hidden' in operation(masked))
    print(operation(Slots()))
    try:
        operation(WrongArity())
    except TypeError:
        print('missing required method argument')
    target = Override()
    target.__dir__ = lambda: ['shadow']
    print(operation(ExceptionDirectory()))
    result = operation(target)
    print(type(result) is list, result, '__dir__' in operation(Override))
    module = __import__(__name__)
    module.__dict__['live_directory_key'] = 1
    print('live_directory_key' in operation(module), operation(module) == sorted(module.__dict__))
    module.__dir__ = lambda: ('last', 'first')
    print(operation(module))
    for bad in (lambda: operation(Broken()), lambda: operation(Mixed()), lambda: operation(1, 2), lambda: operation(obj=1)):
        try:
            bad()
        except TypeError as error:
            print(type(error).__name__, str(error))
