"""Compare data descriptors, callable instances, and attribute fallback with CPython."""


def current():
    order = []

    class Forward:
        def __set_name__(self, owner, name):
            self.name = name
            order.append((owner.__name__, name))

        def __get__(self, instance, owner):
            if instance is None:
                return self
            return getattr(instance.target, self.name)

    class Store:
        def __get__(self, instance, owner):
            return instance.saved

        def __set__(self, instance, value):
            instance.saved = value * 2

    class Raising:
        def __get__(self, instance, owner):
            raise AttributeError("from descriptor")

    class Target:
        def read(self, x):
            return x + 1

        def __call__(self, x, *, extra):
            return x + extra

    class Proxy:
        read = Forward()
        __call__ = Forward()
        write = Store()
        absent = Raising()

        def __init__(self, target):
            self.target = target

        def __getattr__(self, name):
            return "missing " + name

    proxy = Proxy(Target())
    proxy.write = 6
    print(order, Proxy.read.name, proxy.write)
    print(callable(proxy), callable(Proxy), callable(proxy.read), callable(7))
    bound = Target.read.__get__(proxy.target, Target)
    print(bound(3), Target.read.__get__(None, Target) is Target.read)
    print(proxy.read(21), proxy(4, extra=3), proxy.absent, proxy.other)
    proxy.read = 9
    proxy.__call__ = lambda x: x
    print(proxy.read, proxy.__call__(7), proxy(7, extra=8))

    class LateDescriptor:
        pass

    late = LateDescriptor()
    late.write = "instance"
    LateDescriptor.write = Store()
    late.saved = 42
    print(late.write)

    class ReplaceNext:
        def __set_name__(self, owner, name):
            owner.later = None
            for i in range(2500):
                _ = [i, {}]

    class Names:
        first = ReplaceNext()
        later = Forward()

    print(Names.later is None, order[-1])
