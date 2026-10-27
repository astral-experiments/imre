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


def current():
    operation = dir
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
