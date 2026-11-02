def current():
    import unittest

    events = []
    class Manager:
        def __enter__(self):
            events.append("enter")
            return 42
        def __exit__(self, kind, exc, tb):
            events.append(("exit", kind, exc, tb))

    case = unittest.TestCase()
    other = unittest.TestCase()
    assert case._cleanups == [] and case._cleanups is not other._cleanups
    manager = Manager()
    manager.__enter__ = lambda: -1
    case.addCleanup(events.append, "oldest")
    print(case.enterContext(manager))

    def broken(*, message):
        events.append(message)
        case.addCleanup(events.append, "added during cleanup")
        raise ValueError(message)
    case.addCleanup(broken, message="broken")
    print(case.doCleanups(), case.doCleanups(), events)

    class BrokenManager(Manager):
        def __enter__(self):
            raise RuntimeError("cannot enter")
    try:
        case.enterContext(BrokenManager())
    except RuntimeError as exc:
        print(exc.args)
    print(case.doCleanups(), events[-1])

    def interrupt():
        raise KeyboardInterrupt()
    case.addCleanup(events.append, "remaining")
    case.addCleanup(interrupt)
    try:
        case.doCleanups()
    except KeyboardInterrupt:
        print("interrupted", events[-1])
    print(case.doCleanups(), events[-1])

    from unittest.case import _Outcome
    result = unittest.TestResult()
    case._outcome = _Outcome(result)
    case.addCleanup(broken, message="recorded cleanup error")
    assert not case.doCleanups()
    assert not case._outcome.success
    [(failed, detail)] = result.errors
    assert failed is case
    assert "ValueError: recorded cleanup error" in detail
