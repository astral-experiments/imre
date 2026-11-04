import copy
import functools
import pickle


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
    class Callback(functools.partial):
        pass

    for cls in (functools.partial, Callback):
        callback = cls(len, [1])
        reduction = functools.partial.__reduce__(callback)
        assert callback.__reduce_ex__(4) == reduction
        _, _, state = reduction
        assert state[2] is callback.keywords and state[3] is None
        namespace = callback.__dict__
        assert callback.__reduce__()[2][3] is namespace
        callback.label = []
        shallow = copy.copy(callback)
        deep = copy.deepcopy(callback)
        assert type(shallow) is type(deep) is cls
        assert shallow.args is callback.args and shallow.keywords is callback.keywords
        assert shallow.label is callback.label
        assert deep.args[0] is not callback.args[0] and deep.keywords is not callback.keywords
        assert deep.label is not callback.label and deep() == 1

    cycle = functools.partial(len)
    cycle.__setstate__((len, (cycle,), {}, None))
    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        restored = pickle.loads(pickle.dumps(cycle, protocol))
        assert restored.args[0] is restored
    print("partial state restored")
