def current():
    values = []
    value = slice(values)
    values.append(value)
    print(repr(value))
    print(repr(values))
    shared = slice(1, 2, 3)
    print(repr([shared, shared]))
