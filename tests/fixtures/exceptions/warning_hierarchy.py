"""Warnings are ordinary catchable exceptions; each handler tests a different MRO edge."""

class Derived(RuntimeWarning):
    pass


class DerivedEncoding(EncodingWarning):
    pass


class ClientWarning(UserWarning):
    pass


def check() -> None:
    try:
        raise ClientWarning("client")
    except ResourceWarning:
        raise AssertionError("a sibling warning category matched")
    except Warning as problem:
        print(repr(problem))
    try:
        raise RuntimeWarning("runtime")
    except RuntimeError:
        raise AssertionError("RuntimeWarning is not RuntimeError")
    except Warning as problem:
        print(repr(problem))
    try:
        raise EncodingWarning("encoding")
    except Warning as problem:
        print(repr(problem))
    try:
        raise DerivedEncoding("encoding subclass")
    except Warning as problem:
        print(str(problem))
    try:
        raise Derived("subclass")
    except RuntimeWarning as problem:
        print(str(problem))
    try:
        raise Warning("root")
    except RuntimeWarning:
        raise AssertionError("parent is not its subclass")
    except Exception as problem:
        print(repr(problem))
    try:
        raise ValueError("ordinary")
    except Warning:
        raise AssertionError("ordinary exceptions are not warnings")
    except ValueError as problem:
        print(str(problem))


check()
