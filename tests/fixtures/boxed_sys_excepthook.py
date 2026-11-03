import io
import sys


def current():
    hook = sys.__excepthook__
    assert hook is sys.excepthook
    original_stderr = sys.stderr
    capture = io.StringIO()
    try:
        sys.stderr = capture
        assert hook(object(), ValueError("message"), object()) is None
        assert hook(None, 42, None) is None
        assert capture.getvalue() == (
            "ValueError: message\n"
            "TypeError: print_exception(): Exception expected for value, int found\n"
        )

        def first_trace():
            raise RuntimeError("first")

        try:
            first_trace()
        except RuntimeError as err:
            tb = err.__traceback__

        value = LookupError("with frames")
        sys.stderr = None
        hook(None, value, tb)
        assert value.__traceback__ is None
        sys.stderr = capture
        hook(None, value, tb)
        assert value.__traceback__ is tb
        hook(None, value, tb.tb_next)
        assert value.__traceback__ is tb
        assert capture.getvalue().count("Traceback (most recent call last):") == 2
        assert capture.getvalue().endswith("LookupError: with frames\n")

        for call in (lambda: hook(None, value), lambda: hook(None, value, traceback=None)):
            try:
                call()
            except TypeError:
                pass
            else:
                raise AssertionError("accepted wrong arguments")

        called = []
        sys.excepthook = lambda *args: called.append(args)
        sys.excepthook(LookupError, value, tb)
        assert called == [(LookupError, value, tb)]
        assert sys.__excepthook__ is hook
    finally:
        sys.stderr = original_stderr
        sys.excepthook = hook
