from unittest import SkipTest, TestCase, _enterCleanupContext

_module_cleanups = []


def addModuleCleanup(function, /, *args, **kwargs):
    _module_cleanups.append((function, args, kwargs))


def enterModuleContext(cm):
    return _enterCleanupContext(cm, addModuleCleanup)


def doModuleCleanups():
    errors = []
    while _module_cleanups:
        function, args, kwargs = _module_cleanups.pop()
        try:
            function(*args, **kwargs)
        except Exception as exc:
            errors.append(exc)
    if errors:
        raise errors[0]
