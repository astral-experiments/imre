def current():
    row = [1, 2]
    output = [0]

    def change_row():
        row[1] = 99
        return 0

    for output[change_row()], second in [row]:
        assert (output[0], second) == (1, 2)
    pair = (3, 4)
    for output[0], second in [pair]:
        assert (output[0], second) == pair
    assert pair == (3, 4)

    cursor = enumerate(["a", "b"], 3)
    print(cursor.__iter__() is cursor)
    print(cursor.__next__())
    print(enumerate.__iter__(cursor) is cursor)
    print(enumerate.__next__(cursor))
    for method, receiver in (
        (enumerate.__next__, cursor),
        (enumerate.__iter__, 1),
        (enumerate.__next__, 1),
    ):
        try:
            print(method(receiver))
        except Exception as error:
            print(type(error).__name__, str(error))

    for method in (enumerate.__iter__, enumerate.__next__):
        for args, kwargs in (((), {}), ((cursor, 1), {}), ((cursor,), {"extra": 1})):
            try:
                method(*args, **kwargs)
            except TypeError as error:
                print(str(error))

    data = {"first": 1, "last": 2}
    for view in (data, data.keys(), data.values(), data.items()):
        for start in (iter, reversed):
            cursor = start(view)
            step = cursor.__next__
            assert cursor.__iter__() is cursor
            for slot in (cursor.__iter__, type(cursor).__iter__, step, type(cursor).__next__):
                assert slot.__text_signature__ == "($self, /)"
                print(slot.__qualname__, slot.__doc__)
            print(step(), type(cursor).__next__(cursor))
            try:
                step()
            except StopIteration as error:
                print(error.args)
            else:
                raise AssertionError("exhausted cursor produced another value")
    cursor = iter(data)
    step = cursor.__next__
    data["new"] = 3
    try:
        step()
    except RuntimeError as error:
        print(error)
    else:
        raise AssertionError("saved cursor ignored a size change")
