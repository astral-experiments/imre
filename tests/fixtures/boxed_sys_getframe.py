import sys


def caller():
    return sys._getframe(1)


def suspended():
    frame = sys._getframe()
    yield frame, frame.f_back
    yield sys._getframe(), frame.f_back


def overwritten_fast_locals():
    import gc
    import weakref

    class Item:
        pass

    def run():
        fast = Item()
        cell = Item()
        refs = weakref.ref(fast), weakref.ref(cell)
        frame = sys._getframe()
        frame.f_locals.update({'fast': None, 'cell': None})
        gc.collect()
        assert fast is None and refs[0]() is not None and refs[1]() is None
        def read_cell():
            return cell
        assert read_cell() is None
        return frame, refs[0]

    frame, ref = run()
    gc.collect()
    assert ref() is not None
    frame.clear()
    gc.collect()
    assert ref() is None

    def with_nested_class():
        slot = None
        class Nested:
            pass
        return sys._getframe()

    frame = with_nested_class()
    frame.clear()
    proxy = frame.f_locals
    proxy['slot'] = Item()
    ref = weakref.ref(proxy['slot'])
    proxy['slot'] = None
    gc.collect()
    assert ref() is not None
    frame.clear()
    gc.collect()
    assert ref() is None


def current():
    overwritten_fast_locals()
    frame = sys._getframe()
    assert caller() is frame and frame.f_globals is globals()
    assert frame.f_code.co_name == 'current' and frame.f_lineno > frame.f_code.co_firstlineno

    namespace = {'sys': sys}
    exec("captured = sys._getframe()", namespace)

    class Example:
        captured = sys._getframe()
        namespace = locals()

    for captured, original in [(namespace['captured'], namespace), (Example.captured, Example.namespace)]:
        assert captured.f_locals is original
        captured.clear()
        held = captured.f_locals
        assert type(held) is dict and not held and captured.f_locals is held
        held['after'] = 1
        captured.clear()
        assert held == {'after': 1} and captured.f_locals == {}

    def class_cells(x, hidden):
        class Captured:
            y = x
            frame = sys._getframe()

        proxy = type(sys._getframe().f_locals)(Captured.frame)
        assert proxy.copy() == {'x': 13}
        proxy['x'] = 99
        assert x == 99 and Captured.y == 13 and proxy.copy() == {'x': 99}
        proxy['hidden'] = 2
        assert hidden == 1 and 'x' not in Captured.frame.f_locals
        Captured.frame.clear()
        assert not proxy
        proxy['x'] = 37
        Captured.frame.clear()
        assert x == 99 and proxy.copy() == {'x': 37}

        class Shadow:
            x = 7
            frame = sys._getframe()
            def read(self):
                return x

        view = type(proxy)(Shadow.frame)
        assert view['x'] == 99 and Shadow.frame.f_locals['x'] == 7
        view['x'] = 42
        assert Shadow().read() == x == 42 and Shadow.x == 7

        class Generic[T]:
            frame = sys._getframe()

        assert type(proxy)(Generic.frame)['.type_params'] is Generic.__type_params__

    class_cells(13, 1)

    def slot_order(flag):
        if flag:
            zebra = 0
        alpha = 2
        zebra = 1
        return sys._getframe().f_locals

    layout = slot_order(False)
    layout['extra'] = 3
    assert list(layout.keys()) == ['flag', 'zebra', 'alpha', 'extra']
    assert list(layout.values()) == [False, 1, 2, 3]

    def resume(generator):
        return next(generator)

    fresh = suspended()
    fresh_frame = fresh.gi_frame
    fresh_frame.clear()
    assert fresh.gi_frame is None and fresh_frame.f_code is fresh.gi_code
    assert next(fresh, None) is None

    generator = suspended()
    assert generator.__name__ is suspended.__name__
    assert generator.__qualname__ is suspended.__qualname__
    generator.__name__ = "renamed"
    assert suspended.__name__ == "suspended"
    try:
        del generator.__qualname__
    except TypeError:
        pass
    else:
        raise AssertionError("deleting the generator's qualified name succeeded")
    saved = generator.gi_frame
    assert saved.f_code is generator.gi_code and saved.f_lineno == saved.f_code.co_firstlineno
    assert saved.f_back is None and not generator.gi_running and not generator.gi_suspended
    active, first_caller = next(generator)
    assert active is saved and saved.f_lineno == saved.f_code.co_firstlineno + 2
    assert generator.gi_suspended and not generator.gi_running
    try:
        saved.clear()
    except RuntimeError as error:
        assert str(error) == "cannot clear a suspended frame"
    else:
        raise AssertionError("cleared a suspended generator")
    assert first_caller is frame and saved.f_back is None
    same, second_caller = resume(generator)
    assert same is saved and saved.f_back is None
    assert second_caller.f_code is resume.__code__ and second_caller.f_back is frame
    generator.close()
    assert generator.gi_frame is None and not generator.gi_suspended and not generator.gi_running
    assert generator.__name__ == "renamed" and saved.f_back is None
    saved.clear()
    assert saved.f_globals is globals()
    assert sys._getframe.__text_signature__ == '($module, depth=0, /)'
    try:
        sys._getframe(2**40)
    except OverflowError:
        pass
    else:
        raise AssertionError('depth did not fit in a C int')

    def origin():
        return sys._getframemodulename(), sys._getframemodulename(depth=1)

    marker = object()
    origin.__module__ = marker
    assert origin() == (marker, __name__)
    assert sys._getframemodulename(-1) == __name__
    assert sys._getframemodulename(10_000) is None
    assert sys._getframemodulename.__text_signature__ == '($module, /, depth=0)'

    observed = []
    def hook(event, arguments):
        if event == 'object.__getattr__' and arguments == (frame, 'f_code'):
            raise PermissionError('blocked frame code')
        if event == 'sys._getframemodulename' and arguments == (-3,):
            raise PermissionError('blocked module lookup')
        if event == 'sys._getframe':
            observed.extend(arguments)
            raise PermissionError('blocked frame')
    sys.addaudithook(hook)
    assert second_caller.f_back is frame
    try:
        sys._getframe(0)
    except PermissionError as exc:
        assert str(exc) == 'blocked frame' and observed == [frame]
    else:
        raise AssertionError('audit hook veto ignored')
    try:
        frame.f_code
    except PermissionError as exc:
        assert str(exc) == 'blocked frame code'
    else:
        raise AssertionError('frame code audit hook veto ignored')
    try:
        sys._getframemodulename(-3)
    except PermissionError as exc:
        assert str(exc) == 'blocked module lookup'
    else:
        raise AssertionError('module lookup ignored audit veto')
