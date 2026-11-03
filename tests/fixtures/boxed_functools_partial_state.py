import functools


def current():
    def capture(*args, **keywords):
        return args, keywords

    class Namespace(dict):
        pass

    partial = functools.partial(len)
    args = (functools.Placeholder, 2)
    keywords = {"marker": functools.Placeholder}
    namespace = Namespace(label="restored")
    state = (capture, args, keywords, namespace)
    assert functools.partial.__setstate__(partial, state) is None

    def check_state():
        assert partial.func is capture and partial.args is args
        assert partial.keywords is keywords and partial.__dict__ is namespace
        assert partial.label == "restored"
        assert partial(1, extra=3) == ((1, 2), {"marker": functools.Placeholder, "extra": 3})

    check_state()
    conversion_error = RuntimeError("state conversion failed")

    class BrokenArgs(tuple):
        def __iter__(self):
            raise conversion_error

    class BrokenKeywords(dict):
        def __iter__(self):
            return super().__iter__()

        def keys(self):
            raise conversion_error

    for malformed, error_type, message in (
        ((len, (), {}, []), TypeError, "invalid partial state"),
        ((len, (functools.Placeholder,), {}, None), TypeError, "trailing Placeholders are not allowed"),
        ((len, BrokenArgs((0,)), {}, None), RuntimeError, "state conversion failed"),
        ((len, (), BrokenKeywords(a=1), None), RuntimeError, "state conversion failed"),
    ):
        try:
            partial.__setstate__(malformed)
        except (TypeError, RuntimeError) as error:
            assert type(error) is error_type and str(error) == message
            if error_type is RuntimeError:
                assert error is conversion_error
        else:
            raise AssertionError("malformed partial state accepted")
        check_state()
    print("partial state restored")
