class Base:
    def receiver(self):
        return self


class Child(Base):
    pass


Parent = super


class Proxy(Parent):
    pass


child = Child()
proxy = Proxy(Child, child)
assert type(proxy) is Proxy and proxy.receiver() is child
