def selected_method_during_comparison():
    import gc
    import weakref

    events = []

    class Owner:
        def read(self, value):
            assert receiver() is self and id(method()) == method_id
            events.append("method")
            return value

    class Key:
        def __hash__(self):
            return hash("read")

        def __eq__(self, name):
            assert name == "read"
            owners[0].__dict__.clear()
            del Owner.read
            gc.collect()
            assert id(method()) == method_id
            events.append("comparison")
            return False

    def argument():
        owners.clear()
        gc.collect()
        assert receiver() is not None and id(method()) == method_id
        events.append("argument")
        return 41

    owners = [Owner()]
    receiver = weakref.ref(owners[0])
    method = weakref.ref(Owner.read)
    method_id = id(Owner.read)
    owners[0].__dict__[Key()] = None
    assert owners[0].read(argument()) == 41
    assert events == ["comparison", "argument", "method"]
    gc.collect()
    assert receiver() is None and method() is None


def failed_metaclass_comparison():
    events = []
    failure = LookupError("first comparison")

    class Key(str):
        def __hash__(self):
            events.append("hash")
            return hash("__reduce_ex__")

        def __eq__(self, other):
            events.append(other)
            if len(events) == 2:
                raise failure
            return True

    key = Key("another spelling")
    try:
        raise RuntimeError("active handler")
    except RuntimeError as outer:
        try:
            object.__getattribute__(str, key)
        except AttributeError as missing:
            assert missing.name is key and missing.obj is str
            assert missing.__context__ is outer and failure.__context__ is outer
        else:
            raise AssertionError("object lookup retried the class namespace")
        assert events == ["hash", "__reduce_ex__"]

        events.clear()
        assert type.__getattribute__(str, key) is object.__reduce_ex__
        assert events == ["hash", "__reduce_ex__", "hash", "__reduce_ex__"]


def current():
    selected_method_during_comparison()
    failed_metaclass_comparison()
    class Key(str):
        def __hash__(self):
            return hash("payload")

        def __eq__(self, other):
            return other == "payload"

    class Fields:
        @property
        def payload(self):
            return self.saved

        @payload.setter
        def payload(self, value):
            self.saved = value

        @payload.deleter
        def payload(self):
            del self.saved

    assert Fields.__static_attributes__ == ("saved",)

    class Failure(Fields, Exception):
        pass

    key = Key("unrelated")
    for receiver in (Fields(), Failure("reason")):
        setattr(receiver, key, 23)
        assert getattr(receiver, key) == receiver.saved == 23
        object.__delattr__(receiver, key)
        assert not hasattr(receiver, "saved")
        assert not receiver.__dict__

    query = "".join(("dynamic_", "field"))

    class StoredKey(str):
        __hash__ = str.__hash__

        def __eq__(self, other):
            assert other is query
            return str.__eq__(self, other)

    receiver = Fields()
    receiver.__dict__[StoredKey(query)] = 37
    assert getattr(receiver, query) == 37

    class Packet(bytes):
        pass

    class ReadName(str):
        def __hash__(self):
            self.calls += 1
            if self.calls == 2:
                raise RuntimeError("instance dictionary lookup")
            return hash(str(self))

    packet = Packet()
    name = ReadName("fromhex")
    name.calls = 0
    result = getattr(packet, name)("41")
    assert type(result) is Packet and result == b"A" and name.calls == 1
    packet.tag = 1
    del packet.tag
    name.calls = 0
    try:
        getattr(packet, name)
    except RuntimeError as error:
        assert str(error) == "instance dictionary lookup" and name.calls == 2
    else:
        raise AssertionError("lookup skipped the emptied instance dictionary")

    class Bag(set):
        pass

    descriptor = vars(set)["__class_getitem__"]
    method = descriptor.__get__(None, Bag)
    assert type(descriptor).__name__ == "classmethod_descriptor"
    assert type(method) is type(len) and method.__self__ is Bag
    assert method(int).__origin__ is Bag

    error = Exception("state")
    assert error.__reduce__() == (Exception, ("state",))
    try:
        del error.missing
    except AttributeError:
        pass
    else:
        raise AssertionError("deleted an absent exception attribute")
    assert error.__reduce__() == (Exception, ("state",), {})
