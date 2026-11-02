import builtins


class Client:
    def __init__(self, category=DeprecationWarning):
        self.category = category


class Derived(ResourceWarning):
    pass


def current():
    assert Client().category is builtins.DeprecationWarning
    for category in (
        UserWarning, DeprecationWarning, PendingDeprecationWarning, SyntaxWarning,
        RuntimeWarning, FutureWarning, ImportWarning, UnicodeWarning, BytesWarning,
        EncodingWarning, ResourceWarning,
    ):
        assert getattr(builtins, category.__name__) is category
        assert issubclass(category, Warning) and not issubclass(Warning, category)
        error = category("message")
        assert type(error) is category and isinstance(error, Exception)
        try:
            raise error
        except RuntimeError:
            raise AssertionError("warning matched an unrelated exception")
        except Warning as caught:
            assert caught is error and caught.args == ("message",)

    try:
        raise Derived("derived")
    except UserWarning:
        raise AssertionError("warning matched a sibling category")
    except ResourceWarning as caught:
        print(caught)


if __name__ == "__main__":
    current()
