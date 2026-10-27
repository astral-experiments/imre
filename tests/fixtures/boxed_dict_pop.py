class Mapping(dict):
    def __missing__(self, key):
        raise AssertionError("pop called __missing__")

    def __delitem__(self, key):
        raise AssertionError("pop called __delitem__")


def current():
    value = [7]
    data = Mapping(a=value, b=2)
    keys = data.keys()
    print(data.pop("a") is value, list(keys))
    print(dict.pop(data, "missing", value) is value, list(keys))
    print(data.pop("b", None), list(keys))
    for key in ("missing", 7, (1, 2)):
        try:
            data.pop(key)
        except KeyError as error:
            print(error.args, error.args[0] is key)
        try:
            del {}[key]
        except KeyError as error:
            print(error.args, error.args[0] is key)
    print(data.pop([], value) is value)
    try:
        {"a": 1}.pop((0, []), None)
    except TypeError as error:
        print(str(error))
    for method, args in (
        (dict.pop, ()),
        (dict.pop, ([], 1)),
        (data.pop, ()),
        (data.pop, (1, 2, 3)),
    ):
        try:
            method(*args)
        except TypeError as error:
            print(error.args)
    try:
        data.pop("missing", default=None)
    except TypeError as error:
        print(error.args)
