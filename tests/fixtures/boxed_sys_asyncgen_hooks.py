import gc
import sys
import weakref


def current():
    old = sys.get_asyncgen_hooks()
    assert old.firstiter is old[0] is None
    assert old.finalizer is old[1] is None
    assert type(old).__name__ == "asyncgen_hooks"

    class Hook:
        def __call__(self, generator):
            pass

    before = Hook()
    sys.set_asyncgen_hooks(finalizer=before)
    for first, last in [(1, print), (print, 1)]:
        try:
            sys.set_asyncgen_hooks(first, last)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid hook accepted")
        assert sys.get_asyncgen_hooks() == (None, before)

    active = True
    events = []

    def audit(name, args):
        if active and name.startswith("sys.set_asyncgen_hook_"):
            events.append((name, args, sys.get_asyncgen_hooks()))
            if name == "sys.set_asyncgen_hook_firstiter":
                raise ValueError("refuse firstiter")

    sys.addaudithook(audit)
    try:
        sys.set_asyncgen_hooks(print, len)
    except ValueError as error:
        assert str(error) == "refuse firstiter"
    else:
        raise AssertionError("audit veto ignored")
    active = False
    assert events == [
        ("sys.set_asyncgen_hook_finalizer", (), (None, before)),
        ("sys.set_asyncgen_hook_firstiter", (), (None, len)),
        ("sys.set_asyncgen_hook_finalizer", (), (None, len)),
    ]
    assert sys.get_asyncgen_hooks() == (None, before)

    hook = Hook()
    ref = weakref.ref(hook)
    sys.set_asyncgen_hooks(firstiter=hook)
    del hook
    gc.collect()
    assert sys.get_asyncgen_hooks().firstiter is ref() is not None
    sys.set_asyncgen_hooks(*old)
    gc.collect()
    assert ref() is None
    order = []
    class Replaced(Hook):
        def __del__(self):
            order.append("released")

    prior = Replaced()
    ref = weakref.ref(prior)
    sys.set_asyncgen_hooks(finalizer=prior)
    del prior

    def observe(name, args):
        if name.startswith("sys.set_asyncgen_hook_"):
            order.append((name, ref() is not None))

    sys.addaudithook(observe)
    sys.set_asyncgen_hooks(print, len)
    assert order == [("sys.set_asyncgen_hook_finalizer", True),
                     "released", ("sys.set_asyncgen_hook_firstiter", False)]
    sys.set_asyncgen_hooks(*old)
    print("asyncgen hooks")
