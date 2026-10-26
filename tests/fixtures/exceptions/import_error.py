class Missing(ModuleNotFoundError):
    pass


def describe(err: ImportError) -> None:
    print(str(err))
    print(repr(err))
    print(repr(err.args))
    print(len(err.args))
    print(repr(err.msg))
    print(repr(err.name))
    print(repr(err.path))
    print(repr(err.name_from))
    print(f"{err!r}: {err.msg!r}")


def check_guard() -> None:
    try:
        raise ImportError('support must be imported from the test package')
    except ModuleNotFoundError:
        print("unexpected sibling")
    except ImportError as err:
        print(err.args == ('support must be imported from the test package',))
        print(err.name is None)
        print(err.path is None)
        print(isinstance(err, Exception))
        print(isinstance(err, OSError))
        print(str(err))
        print(repr(err))


def record(n: int, calls: list[int]) -> int:
    calls.append(n)
    return n


def expanded_keywords() -> None:
    calls: list[int] = []
    try:
        ImportError(
            record(1, calls), name=record(2, calls),
            **{'name': record(3, calls), 'path': record(4, calls)},
            **{'name_from': record(5, calls)},
        )
    except TypeError as err:
        print(str(err))
    print(repr(calls))
    # Replacing an entry in one display still evaluates both values.
    describe(ImportError(**{'name': record(6, calls), 'name': record(7, calls)}))
    describe(ImportError(**{'name': [0], 'name': 'retained'}))
    print(repr(calls))
    try:
        Missing(name=record(8, calls), **{'name': record(9, calls)}, **{'path': record(10, calls)})
    except TypeError as err:
        print(str(err))
    print(repr(calls))
    try:
        ModuleNotFoundError(**{'path': 'one'}, **{'path': 'two'})
    except TypeError as err:
        print(str(err))
    try:
        ImportError(**{1: 'bad'})
    except TypeError as err:
        print(str(err))
    try:
        ImportError(**1)
    except TypeError as err:
        print(str(err))
    old = ImportError('old', name='pkg', path='file', name_from='symbol')
    try:
        old.__init__('new', **{'name': 'first'}, name='second')
    except TypeError as err:
        print(str(err))
    describe(old)
    try:
        old.__init__('new', **{'name': 'first', 'unexpected': 'second'})
    except TypeError as err:
        print(str(err))
    describe(old)


def exercise() -> None:
    describe(ImportError())
    describe(ImportError(**{'name': 'pkg'}))
    describe(ModuleNotFoundError('missing', **{'path': b'archive'}, **{'name': 'pkg', 'name_from': 'symbol'}))
    describe(ImportError(None))
    describe(ImportError(b'x\xff', name='a.b', path='m.py', name_from='b'))
    describe(ImportError('bad', 1237940039285380274899124224, False, name=b'name', path=None, name_from=(3,)))
    old = ModuleNotFoundError('old', path='archive')
    previous = old.args
    old.__init__('new', name='mod')
    print(repr(previous))
    print(repr(old.args))
    old.__init__()
    print(old.args == ())
    print(old.name is None)
    print(old.path is None)
    print(old.msg is None)
    for message in ["original", ""]:
        old.__init__(message, name="pkg", path="module.py", name_from="symbol")
        prior = old.args
        seen: list[int] = []
        try:
            old.__init__(
                record(1, seen), record(2, seen),
                unexpected=record(3, seen), name=record(4, seen),
            )
        except TypeError:
            print(repr(seen))
            print(repr(prior))
            describe(old)
    old.__init__(42)
    try:
        old.__init__("new", unexpected=True)
    except TypeError:
        describe(old)
    try:
        raise Missing('missing', name='pkg')
    except OSError:
        print("unexpected")
    except ImportError as err:
        print(isinstance(err, Missing))
        print(isinstance(err, ModuleNotFoundError))
        print(repr(err))
        print(err.name == "pkg")
    try:
        raise ImportError
    except Exception as err:
        if isinstance(err, ImportError):
            print(repr(err.args))
    calls: list[int] = []
    try:
        ImportError(record(1, calls), path=record(2, calls), wrong=record(3, calls), other=record(4, calls))
    except TypeError as err:
        print(str(err))
    print(repr(calls))
    numbered=ImportError(record(5,calls), path=record(6,calls), name=record(7,calls))
    print(repr(calls))
    print(repr(numbered.name))
    print(repr(numbered.path))
    parts=ImportError(True, 'éx', b'az', (False, 4)).args
    print(parts == (1, 'éx', b'az', (0, 4)))
    print(parts == ('1', 'éx', b'az', (0, 4)))
    print(isinstance(parts, tuple))
    print(isinstance(parts[0], int))
    print(isinstance(parts[2], bytes))
    print(repr(parts[1][-2]))
    print(repr(parts[2][-1]))
    print(repr(parts[-1]))
    try:
        print(repr(parts[100]))
    except IndexError as err:
        print(str(err))
    try:
        print(len(parts[0]))
    except TypeError as err:
        print(str(err))


check_guard()
expanded_keywords()
exercise()
