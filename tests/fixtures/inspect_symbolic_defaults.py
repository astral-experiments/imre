from inspect import getfullargspec, signature


class Defaults:
    retries = 3


def send(path, /, *, attempts=1):
    return path


send.__text_signature__ = "(path, /, *, attempts=Defaults.retries)"
assert str(signature(send)) == "(path, /, *, attempts=3)"
Defaults.retries = 7
assert getfullargspec(send).kwonlydefaults == {"attempts": 7}

send.__text_signature__ = "(path, /, *, debug=__debug__)"
print(signature(send).parameters["debug"].default)
