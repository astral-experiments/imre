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


class EditList:
    def __init__(self, target):
        self.target = target

    def __reduce__(self):
        self.target.append("added")
        return int, (7,)


def list_cases():
    singleton = []
    singleton.append(EditList(singleton))
    live = [None, "second"]
    live[0] = EditList(live)
    shared = []
    shared.append(shared)
    return singleton, live, [shared, shared], BadReturn((iter, (["c", "a"],)))


class RestoredMapping(dict):
    def __reduce__(self):
        return type(self), (), self.state, None, iter(self.items())

    def __setstate__(self, state):
        assert list(self) == ["z", "a", "m"]
        assert self["z"] is self and self["a"] is state
        self.state = state


def mapping_items_case(protocol):
    for items, error in (([], pickle.PicklingError), (iter([["a", 1]]), TypeError),
                         (iter([("a",)]), TypeError)):
        try:
            pickle.dumps(BadReturn((dict, (), None, None, items)), protocol)
        except error:
            pass
        else:
            raise AssertionError("accepted invalid mapping item iterator")
    mapping = RestoredMapping()
    mapping["z"] = mapping
    mapping["a"] = {}
    mapping["m"] = 42
    mapping.state = mapping["a"]
    restored = pickle.loads(pickle.dumps(mapping, protocol))
    assert restored["z"] is restored and restored.state is restored["a"]
    return mapping


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
    assert pickle._loads is not pickle.loads
    for stream in streams:
        for reader in (pickle.loads, pickle._loads):
            obj, label = reader(stream)
            assert type(obj) is LabeledTuple and obj == (3, 4)
            assert obj.restored and obj.label is label and label["back"] is obj
    assert pickle._loads(b"N.", encoding=None) is None
    for bad in (b".", b"!", b"h\x07.", b"N]b.", b"cbuiltins\nlist\n]\x81."):
        try:
            pickle.loads(bad)
        except pickle.UnpicklingError:
            pass
        else:
            raise AssertionError("accepted invalid pickle")


class Plain:
    __slots__ = ("slot", "__dict__")

    def __init__(self, item):
        self.slot = self.link = item


def check_object_reducer():
    class Child(WithoutProtocol):
        def reduce_from_super(self):
            return super().__reduce_ex__(4)

    class Mutable(set):
        pass

    class Frozen(frozenset):
        pass

    assert Mutable.__reduce__ is set.__reduce__
    assert Frozen.__reduce__ is frozenset.__reduce__
    obj = Child()
    assert Child.__reduce_ex__ is object.__reduce_ex__
    assert obj.reduce_from_super() == (str, (37,))
    obj.__reduce__ = lambda: (int, (12,))
    assert object.__reduce_ex__(obj, 2) == (int, (12,))

    events = []
    class Reducer:
        def __get__(self, instance, owner):
            events.append(instance is None)
            if instance is None:
                raise LookupError("class reducer lookup")
            return lambda: (str, (37,))

    class WithDescriptor:
        __reduce__ = Reducer()

    try:
        WithDescriptor().__reduce_ex__(5)
    except LookupError as error:
        assert str(error) == "class reducer lookup"
    else:
        raise AssertionError("ignored an error inspecting the class reducer")
    assert events == [False, True]

    import copy
    import copyreg

    class Legacy:
        def __getnewargs__(self):
            raise AssertionError("legacy reduction requested new arguments")

    legacy = Legacy()
    for protocol in (0, 1):
        assert legacy.__reduce_ex__(protocol) == (copyreg._reconstructor, (Legacy, object, None))
        legacy.value = [7]
        constructor, args, state = legacy.__reduce_ex__(protocol)
        assert constructor is copyreg._reconstructor and args == (Legacy, object, None)
        assert state is legacy.__dict__ and state["value"] is legacy.value
        del legacy.value
        try:
            Plain({}).__reduce_ex__(protocol)
        except TypeError as error:
            assert "__slots__" in str(error)
        else:
            raise AssertionError("legacy reduction accepted slots without a custom state hook")

    plain = Plain({})
    for protocol in (2, 3, 4, 5):
        constructor, args, state, listitems, dictitems = plain.__reduce_ex__(protocol)
        assert constructor is copyreg.__newobj__ and args == (Plain,)
        assert state[0] is plain.__dict__ and state[1]["slot"] is plain.slot
        assert listitems is None and dictitems is None
        restored = pickle.loads(pickle.dumps(plain, protocol))
        assert type(restored) is Plain and restored.link is restored.slot
    for args, message in (((), "expected at least 1 argument"),
                          ((23,), "must be a class"),
                          ((Plain,), "first argument to __newobj__() must be")):
        try:
            pickle.dumps(BadReturn((copyreg.__newobj__, args)), 4)
        except pickle.PicklingError as error:
            assert message in str(error)
        else:
            raise AssertionError("accepted invalid __newobj__ arguments")
    clone = copy.copy(plain)
    assert type(clone) is Plain and clone is not plain
    assert clone.__dict__ is not plain.__dict__
    assert clone.link is clone.slot is plain.slot


class Outer:
    class Middle:
        class Inner:
            pass


def check_global_names(protocol):
    from collections import OrderedDict, UserDict, UserList, UserString
    from datetime import date, datetime, timedelta

    for cls in (date, datetime, timedelta):
        assert cls.__module__ == "datetime" and cls.__qualname__ == cls.__name__
        assert pickle.loads(pickle.dumps(cls, protocol)) is cls

    for cls in (Outer.Middle, Outer.Middle.Inner, OrderedDict, UserDict, UserList, UserString):
        for fix_imports in (True, False):
            data = pickle.dumps(cls, protocol, fix_imports=fix_imports)
            assert pickle.loads(data, fix_imports=fix_imports) is cls


def produce():
    check_object_reducer()
    assert _codecs.encode is codecs.encode and codecs.encode.__module__ == "_codecs"
    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        check_global_names(protocol)
        check_reduction_state(protocol)
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
                 (b"", payload[:-1], payload, payload, ReducedBytes(b"x")), list_cases(), mapping_items_case(protocol))
        print(list(pickle.dumps(value, protocol)))


class Stateful:
    def __init__(self, links):
        self.links = links
        self.restorations = 0

    def __reduce__(self):
        return type(self), (self.links,), self.state

    def __setstate__(self, state):
        self.state = state
        self.restorations += 1


def check_reduction_state(protocol):
    links = {}
    obj = Stateful(links)
    links["owner"] = obj
    obj.state = {"links": links, "owner": obj}
    restored, state = pickle.loads(pickle.dumps((obj, obj.state), protocol))
    assert restored.state is state and restored.restorations == 1
    assert state["owner"] is restored and state["links"] is restored.links
    assert restored.links["owner"] is restored
