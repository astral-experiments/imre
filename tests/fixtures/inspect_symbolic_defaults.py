import inspect


class Defaults:
    retries = 3


def send(*args, **kwargs):
    pass


send.__text_signature__ = "(path, /, *, attempts=Defaults.retries)"
assert str(inspect.signature(send)) == "(path, /, *, attempts=3)"
Defaults.retries = 7
assert inspect.getfullargspec(send).kwonlydefaults == {"attempts": 7}
