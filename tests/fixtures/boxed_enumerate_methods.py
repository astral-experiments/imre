def current():
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
