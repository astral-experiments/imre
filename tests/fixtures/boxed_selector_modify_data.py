def raising_comparison(fd):
    from selectors import PollSelector, EVENT_READ, EVENT_WRITE
    log = []
    class Data:
        def __ne__(self, other):
            log.append(self.name)
            if self.name == "new":
                raise ValueError("comparison")
            return True
    old = Data()
    old.name = "old"
    new = Data()
    new.name = "new"
    with PollSelector() as selector:
        key = selector.register(fd, EVENT_READ, old)
        try:
            selector.modify(fd, EVENT_WRITE, new)
        except ValueError as error:
            print(str(error))
        print(log, selector.get_key(fd) is key, selector.get_key(fd).events)


from selectors import PollSelector, EVENT_READ, EVENT_WRITE

def current():
    first = 0
    raising_comparison(first)
    calls = []
    class Base:
        def __ne__(self, other):
            calls.append("base-ne")
            return NotImplemented
    class Derived(Base):
        def __ne__(self, other):
            calls.append("derived-ne")
            return False

    old = Derived()
    with PollSelector() as selector:
        key = selector.register(first, EVENT_READ, old)
        print(selector.modify(first, EVENT_READ, Base()) is key, calls)
        print(selector.modify(first, EVENT_WRITE, Base()).events, calls)

    class Fallback:
        def __eq__(self, other):
            calls.append("eq")
            return True

    with PollSelector() as selector:
        key = selector.register(first, EVENT_READ, Fallback())
        print(selector.modify(first, EVENT_READ, Fallback()) is key, calls)
