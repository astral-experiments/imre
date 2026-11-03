import ssl


class GuardedContext(ssl.SSLContext):
    def __getattribute__(self, name):
        if name == "_ctx":
            raise AssertionError("session_stats read a subclass attribute")
        return super().__getattribute__(name)


for protocol in (ssl.PROTOCOL_TLS_CLIENT, ssl.PROTOCOL_TLS_SERVER):
    ctx = GuardedContext(protocol)
    snapshot = ctx.session_stats()
    assert type(snapshot) is dict
    assert all(type(value) is int for value in snapshot.values())
    print(list(snapshot.items()))
    snapshot.clear()
    assert ctx.session_stats()["connect"] == 0


server = GuardedContext(ssl.PROTOCOL_TLS_SERVER)
client = GuardedContext(ssl.PROTOCOL_TLS_CLIENT)


class TicketCount:
    def __index__(self):
        server.num_tickets = 1
        assert server.num_tickets == 1
        return 3


server.num_tickets = TicketCount()
assert server.num_tickets == 3
assert client.num_tickets == 2
try:
    client.num_tickets = -1
except ValueError as exc:
    assert str(exc) == "value must be non-negative"
else:
    raise AssertionError("accepted a negative ticket count")
try:
    client.num_tickets = 1 << 100
except OverflowError:
    pass
else:
    raise AssertionError("accepted a ticket count outside signed long range")
