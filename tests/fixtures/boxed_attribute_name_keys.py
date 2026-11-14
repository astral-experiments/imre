def current():
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
