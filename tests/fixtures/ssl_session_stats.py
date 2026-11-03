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
