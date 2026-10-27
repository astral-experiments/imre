"""Check OSError's argument tuple and independent operating-system fields."""


def current():
    import errno
    from _weakref import ref

    simple = OSError('short read')
    print(simple.args, simple.errno, simple.strerror, simple.filename, simple.filename2)
    print(str(simple) is simple.args[0])
    missing = OSError(errno.ENOENT, 'missing', 'source', None, 'destination')
    print(type(missing).__name__, missing.args, missing.filename, missing.filename2, str(missing))
    wait = OSError(errno.EAGAIN, 'wait', 3)
    print(type(wait).__name__, wait.args, wait.characters_written, wait.filename)

    class Forwarded(OSError):
        def __init__(self, description):
            super().__init__(errno.EPIPE, description, 'pipe')

    forwarded = Forwarded('closed')
    print(type(forwarded).__name__, forwarded.args, forwarded.filename, str(forwarded))

    events = []

    class Path:
        def __repr__(self):
            events.append('repr path')
            return '<path>'

    class Description:
        def __str__(self):
            events.append('str description')
            failure.filename = None
            for i in range(12000):
                unused = [i]
            return 'description'

    path = Path()
    failure = OSError(errno.ENOENT, Description(), path)
    print(events, failure.filename is path)
    print(str(failure), events, failure.filename)
    owner = OSError(errno.EINVAL, 'bad', Path())
    saved = ref(owner.filename)
    BaseException.__init__(owner, 'replacement')
    for i in range(12000):
        unused = [i]
    print(owner.args, saved() is owner.filename, str(owner))

    class Count:
        def __index__(self):
            owner.filename = 'changed during conversion'
            return 4

    owner.characters_written = Count()
    print(owner.characters_written, owner.filename, owner.args is owner.args)
    OSError.__init__(forwarded, errno.EINVAL, 'retry')
    print(forwarded.args, forwarded.filename, str(forwarded))
    try:
        open(__file__ + '/child', 'rb')
    except OSError as native:
        print(native.args is native.args, native.strerror is native.args[1], native.filename is native.filename)

    description = 'tail'
    chain = []
    for i in range(300):
        description = OSError(2, description)
        chain.append(description)
    print(len(str(description)))
    for value in chain:
        value.strerror = None
        value.args = ()

    recursive = OSError(2, 'first')
    recursive.strerror = recursive
    try:
        str(recursive)
    except RecursionError:
        recursive.strerror = 'again'
    print(str(recursive))
