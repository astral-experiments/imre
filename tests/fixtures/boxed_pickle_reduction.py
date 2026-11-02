import pickle


def rebuild(protocol):
    return ("rebuilt", protocol)


class WithProtocol:
    def __reduce_ex__(self, protocol):
        return rebuild, (protocol,)

    def __reduce__(self):
        raise AssertionError("called __reduce__ after __reduce_ex__")


class WithoutProtocol:
    def __reduce__(self):
        return str, (37,)


class BadReturn:
    def __init__(self, result):
        self.result = result

    def __reduce__(self):
        return self.result


class Denied:
    def __reduce_ex__(self, protocol):
        raise TypeError("reduction denied")

    def __reduce__(self):
        raise AssertionError("fell back after a failing __reduce_ex__")


class EditDictionary:
    def __init__(self, mapping):
        self.mapping = mapping

    def __reduce__(self):
        self.mapping["later"] = 37
        return int, (5,)


def dictionary_cases(protocol):
    growing = {"first": None, "second": 0}
    growing["first"] = EditDictionary(growing)
    try:
        pickle.dumps(growing, protocol)
    except RuntimeError as error:
        assert str(error) == "dictionary changed size during iteration"
    else:
        raise AssertionError("ignored size change during reduction")

    replaced = {"first": None, "later": 0}
    replaced["first"] = EditDictionary(replaced)
    shared = {}
    graph = {"left": shared, "right": shared, "distinct": {}}
    graph["self"] = graph
    small = ({},)
    wide = ({}, 1, 2, 3)
    small[0]["back"] = small
    wide[0]["back"] = wide
    refs = {}
    recursive = BadReturn((tuple, ((refs,),)))
    refs["back"] = recursive
    return graph, small, wide, replaced, recursive


def check_floats(protocol):
    for value, text, bits in (
        (-0.0, b"-0.0", "8000000000000000"),
        (900719925474099.25, b"900719925474099.2", "430999999999999a"),
        (5e-324, b"5e-324", "0000000000000001"),
        (float("-inf"), b"-inf", "fff0000000000000"),
        (-float("nan"), b"nan", "fff8000000000000"),
    ):
        data = pickle.dumps(value, protocol)
        if protocol == 0:
            assert data == b"F" + text + b"\n."
        else:
            assert data.endswith(b"G" + bytes.fromhex(bits) + b".")


def produce():
    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        check_floats(protocol)
        for result in (11, (int,), (11, ()), (int, 11)):
            try:
                pickle.dumps(BadReturn(result), protocol)
            except pickle.PicklingError:
                pass
            else:
                raise AssertionError("accepted invalid __reduce__ result")
        try:
            pickle.dumps(Denied(), protocol)
        except TypeError as error:
            assert str(error) == "reduction denied"
        else:
            raise AssertionError("ignored hook error")
        shared = ("\0\n\r\\\x1aéĀ🍏", 1 << 2048, -2147483649, True, None)
        reduced = WithProtocol()
        value = (shared, shared, reduced, reduced, WithoutProtocol(), (), dictionary_cases(protocol))
        print(list(pickle.dumps(value, protocol)))
