import sys


def caller():
    return sys._getframe(1)


def suspended():
    frame = sys._getframe()
    yield frame
    yield sys._getframe()


def current():
    frame = sys._getframe()
    assert caller() is frame and frame.f_globals is globals()
    assert frame.f_code.co_name == 'current' and frame.f_lineno > frame.f_code.co_firstlineno
    generator = suspended()
    saved = next(generator)
    assert next(generator) is saved
    generator.close()
    saved.clear()
    assert saved.f_globals is globals()
    assert sys._getframe.__text_signature__ == '($module, depth=0, /)'
    try:
        sys._getframe(2**40)
    except OverflowError:
        pass
    else:
        raise AssertionError('depth did not fit in a C int')

    observed = []
    def hook(event, arguments):
        if event == 'sys._getframe':
            observed.extend(arguments)
            raise PermissionError('blocked frame')
    sys.addaudithook(hook)
    try:
        sys._getframe(0)
    except PermissionError as exc:
        assert str(exc) == 'blocked frame' and observed == [frame]
    else:
        raise AssertionError('audit hook veto ignored')
