def exercise(path):
    def buffered(saved=None):
        stream = open(path, "w")
        stream.write("loop")
        if saved is not None:
            saved.append(stream)
        return stream

    def observe_loop_exit(reader, saved, stop_early):
        for value in [None, buffered(saved)]:
            if value is None:
                yield None
                if stop_early:
                    break
            value = None
        yield reader.read()

    for keep in (False, True):
        saved = []
        entries = {"writer": buffered(saved if keep else None), b"loop": 1, b"": 2}
        with open(path, "rb") as reader:
            del entries["writer"], entries[reader.read()]
        assert (b"loop" in entries) == keep
        if keep:
            saved[0].close()

        for stop_early in (False, True):
            saved = []
            with open(path, "rb") as reader:
                assert list(observe_loop_exit(reader, saved if keep else None, stop_early)) == [
                    None, b"" if keep else b"loop"
                ]
            if keep:
                saved[0].close()

        saved = []
        for stream in [buffered()]:
            if keep:
                saved.append(stream)
            stream = None
        else:
            with open(path, "rb") as reader:
                assert reader.read() == (b"" if keep else b"loop")
        if keep:
            saved[0].close()

        saved = []
        try:
            _first, _second = [buffered(saved if keep else None)]
        except ValueError:
            with open(path, "rb") as reader:
                assert reader.read() == (b"" if keep else b"loop")
        else:
            raise AssertionError("expected unpacking failure")
        if keep:
            saved[0].close()

    def finish():
        stream = buffered()
        stream.write("!")

    with open(path, "rb") as reader:
        assert (finish(), reader.read()) == (None, b"loop!")

    with open(path, "rb") as reader:
        assert (buffered() and reader.read()) == b"loop"
    with open(path, "rb") as reader:
        assert (bool(buffered()), reader.read()) == (True, b"loop")
    selected = buffered() or None
    with open(path, "rb") as reader:
        assert reader.read() == b""
    selected.close()

    def checked_index():
        with open(path, "rb") as reader:
            assert reader.read() == b"loop"
        return 0

    slots = [None]
    held = buffered()
    held = slots[checked_index()] = None
    held, held, slots[checked_index()] = buffered(), None, None

    held = buffered()
    held = None
    checked_index()
    held = buffered()
    with open(path, "rb") as reader:
        assert ((held := None), reader.read()) == (None, b"loop")
    held = buffered()
    with open(path, "rb") as reader:
        earlier = (held, (held := None), reader.read())
    assert earlier[2] == b""
    earlier[0].close()

    def writer_phases(saved):
        writer = buffered(saved)
        yield 0
        yield 1

    for keep in (False, True):
        saved = []
        for phase in writer_phases(saved if keep else None):
            with open(path, "rb") as reader:
                assert reader.read() == b""
            break
        with open(path, "rb") as reader:
            assert reader.read() == (b"" if keep else b"loop")
        if keep:
            saved[0].close()

    sample = "Málaga 東京🧪e\u0301\n"
    with open(path, "w") as ready:
        alias = ready
        print(type(ready), type(ready) is type(ready), isinstance(ready, type(ready)))
        print(ready.closed, ready.__enter__() is ready)
        print(ready.write(""), ready.write(sample))
        with open(path, "rb") as observer:
            print(observer.read() == b"")
        print(ready.flush() is None)
        with open(path, "rb") as observer:
            print(observer.read().decode("utf-8") == sample)
        for value in (b"wrong", bytearray(b"wrong"), ready):
            try:
                ready.write(value)
            except TypeError as error:
                print(error)
        try:
            ready.write()
        except TypeError as error:
            print(error)
        try:
            ready.write(s="wrong")
        except TypeError as error:
            print(error)
        try:
            ready.flush(1)
        except TypeError as error:
            print(error)
    print(alias.closed, alias.close() is None)
    try:
        alias.write("closed")
    except ValueError as error:
        print(error)
    try:
        alias.flush()
    except ValueError as error:
        print(error)
    try:
        with alias:
            pass
    except ValueError as error:
        print(error)

    try:
        with open(path, "w") as output:
            print(type(output) is type(alias))
            output.write("4321")
            raise ValueError("body")
    except ValueError as error:
        print(error, output.closed)
    with open(path, "rb") as observer:
        print(observer.read() == b"4321")
    try:
        open("missing-parent/port", "w")
    except FileNotFoundError as error:
        print(error, error.errno, error.strerror, error.filename, error.filename2, error.args)
    try:
        open(path + "/child", "w")
    except OSError as error:
        print(isinstance(error, NotADirectoryError), error.errno, error.filename, error.args)
    try:
        open(".", "w")
    except OSError as error:
        print(isinstance(error, IsADirectoryError), error, error.errno, error.strerror,
              error.filename, error.filename2, error.args)


    with open(path, "w", encoding="utf-8") as output:
        output.write("h\u00e9llo\r\nsecond\rthird\n")
    with open(path) as reader:
        alias = reader
        print(repr(alias.readline()), repr(reader.read()), repr(alias.read()))
    print(alias.closed)
    for mode in ("r", "rt"):
        with open(path, mode, encoding="utf-8") as reader:
            print(repr(reader.read()), repr(reader.readline()))


def full_device():
    output = open("/dev/full", "w")
    print(output.write("x"))
    try:
        output.flush()
    except OSError as error:
        print(error, output.closed, error.errno, error.strerror, error.filename, error.args)
    try:
        output.close()
    except OSError as error:
        print(error, output.closed, error.errno, error.strerror, error.filename, error.args)
    print(output.closed, output.close() is None)
