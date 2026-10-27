def example(a, b=2, /, c=3, *, d=4, e=5):
    return a, b, c, d, e


def current():
    print(example.__defaults__, example.__kwdefaults__)
    example.__defaults__ = (8, 9)
    example.__kwdefaults__ = {'d': 10, 'e': 11}
    print(example(1))
    example.__kwdefaults__['d'] = 12
    print(example(1))
    del example.__defaults__
    del example.__kwdefaults__
    print(example.__defaults__, example.__kwdefaults__)
    try:
        example(1)
    except TypeError:
        print('required')
    print(example(1, 2, 3, d=4, e=5))
    for name in ('__defaults__', '__kwdefaults__'):
        try:
            setattr(example, name, 42)
        except TypeError:
            print(name, 'type error')
