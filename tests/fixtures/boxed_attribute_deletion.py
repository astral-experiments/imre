class Descriptor:
    def __get__(self, obj, owner):
        raise AssertionError('deletion evaluated the getter')

    def __delete__(self, obj):
        obj.events.append('descriptor')


class Subject:
    descriptor = Descriptor()
    constant = 1

    def __delattr__(self, name):
        self.events.append(name)
        object.__delattr__(self, name)


def function():
    pass


def current():
    obj = Subject()
    obj.events = []
    obj.value = 1
    del obj.value
    delattr(obj, 'descriptor')
    object.__delattr__(obj, 'descriptor')
    print(obj.events, hasattr(obj, 'value'))
    try:
        del obj.constant
    except AttributeError as error:
        print(str(error))
    del Subject.constant
    print(hasattr(Subject, 'constant'))

    wrapper = staticmethod(function)
    for target in (wrapper, function):
        namespace = target.__dict__
        namespace['extra'] = 42
        del target.extra
        print('extra' in namespace)
        try:
            delattr(target, 'extra')
        except AttributeError as error:
            print(str(error))
    for target, name in ((wrapper, '__func__'), (function, '__globals__')):
        try:
            delattr(target, name)
        except AttributeError as error:
            print(str(error))
    for args, kwargs in (((), {}), ((obj,), {}), ((obj, 3), {}), ((obj,), {'name': 'x'})):
        try:
            delattr(*args, **kwargs)
        except TypeError as error:
            print(str(error))
