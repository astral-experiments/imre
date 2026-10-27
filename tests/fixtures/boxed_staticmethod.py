def current():
    def echo(value: int, *, label="one"):
        """Return the argument and its label."""
        return value, label

    wrapper = staticmethod(echo)
    print(type(wrapper) is staticmethod, isinstance(wrapper, staticmethod), callable(wrapper))
    print(wrapper.__func__ is echo, wrapper.__wrapped__ is echo, wrapper.__name__ == echo.__name__)
    print(wrapper.__module__ == echo.__module__, wrapper.__qualname__ == echo.__qualname__, wrapper.__doc__ == echo.__doc__)
    print("__annotate__" in wrapper.__dict__, "__annotations__" in wrapper.__dict__)
    print(wrapper.__annotate__ is echo.__annotate__, wrapper.__annotations__ is echo.__annotations__)
    print(wrapper(2, label="two"))

    class Base:
        call = wrapper
        @staticmethod
        def local(*args):
            return args

    class Child(Base):
        pass

    child = Child()
    print(Base.call is echo, Child.call is echo, child.call is echo, child.call(3, label="kw"))
    print(Base.local(1), child.local(1), wrapper.__get__(None, Child) is echo)
    child.call = "shadow"
    print(child.call, Base.call is echo)
    wrapper.tag = 5
    print(wrapper.__dict__["tag"], hasattr(echo, "tag"))
    class DictSubclass(dict):
        def __getitem__(self, key):
            raise AssertionError("wrapper lookup called __getitem__")
        def __setitem__(self, key, value):
            raise AssertionError("wrapper assignment called __setitem__")

    replacement = DictSubclass(tag=8)
    wrapper.__dict__ = replacement
    print(wrapper.tag, wrapper.__dict__ is replacement)
    wrapper.tag = 9
    print(wrapper.tag)
    for name in ("__func__", "__wrapped__"):
        try:
            setattr(wrapper, name, 1)
        except AttributeError as exc:
            print(str(exc))
    try:
        wrapper.__get__(None, None)
    except TypeError as exc:
        print(str(exc))
    empty = staticmethod(None)
    print(callable(empty), empty.__get__(None, Base) is None, empty.__dict__ == {"__doc__": None.__doc__})
    try:
        empty()
    except TypeError as exc:
        print(str(exc))
    for call in (lambda: staticmethod(), lambda: staticmethod(1, 2),
                 lambda: staticmethod(callable=None), lambda: staticmethod.__get__(42, 1)):
        try:
            call()
        except TypeError as exc:
            print(str(exc))
