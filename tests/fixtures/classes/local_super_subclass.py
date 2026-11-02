def factory():
    class Proxy(super):
        pass

    return Proxy


class Child:
    pass


child = Child()
proxy = factory()(Child, child)
assert proxy.__self__ is child
assert type(proxy).__qualname__ == "factory.<locals>.Proxy"
