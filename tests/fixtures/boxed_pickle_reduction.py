import _codecs
import codecs
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


class ReducedBytes(bytes):
    def __reduce_ex__(self, protocol):
        return rebuild, (protocol,)


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


class LabeledTuple(tuple):
    def __setstate__(self, state):
        self.label = state["label"]
        self.restored = True


def consume(streams):
    for stream in streams:
        obj, label = pickle.loads(stream)
        assert type(obj) is LabeledTuple and obj == (3, 4)
        assert obj.restored and obj.label is label and label["back"] is obj
    for bad in (b".", b"!", b"h\x07.", b"N]b.", b"cbuiltins\nlist\n]\x81."):
        try:
            pickle.loads(bad)
        except pickle.UnpicklingError:
            pass
        else:
            raise AssertionError("accepted invalid pickle")


def produce():
    assert _codecs.encode is codecs.encode and codecs.encode.__module__ == "_codecs"
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
        payload = bytes(range(256))
        value = (shared, shared, reduced, reduced, WithoutProtocol(), (), dictionary_cases(protocol),
                 (b"", payload[:-1], payload, payload, ReducedBytes(b"x")))
        print(list(pickle.dumps(value, protocol)))
