import inspect
from functools import update_wrapper
from inspect import Parameter, Signature


def omitted(value=inspect._sentinel):
    return value is inspect._sentinel


def original(a, /, b: int = 3, *args, flag=False, **kwargs) -> str:
    return str(a)


def pair(a, b):
    pass


def wrapper(*values):
    return original(*values)


class C:
    def method(self, /, value: bytes = b"x"):
        pass



class CallableInstance:
    def __call__(self, item, /, *, count=1):
        return item * count

    def __eq__(self, other):
        raise AssertionError("signature compared the callable")


class StaticCall:
    __call__ = staticmethod(lambda first, second: None)


class ClassCall:
    @classmethod
    def __call__(cls, item):
        return item


target = CallableInstance()
target.__call__ = lambda shadow: None
assert str(inspect.signature(target)) == "(item, /, *, count=1)"
assert str(inspect.signature(StaticCall())) == "(first, second)"
assert str(inspect.signature(ClassCall())) == "(item)"


class InheritedName(str):
    pass


class DeniedName(str):
    def isidentifier(self):
        return False


class Published:
    def __init__(self):
        self.reads = 0

    @property
    def __signature__(self):
        self.reads += 1
        return Signature()

    def __call__(self):
        pass


update_wrapper(wrapper, original)
sig = Signature.from_callable(wrapper)
print(str(sig), sig.return_annotation is str)
print([(p.name, p.kind.name) for p in sig.parameters.values()])
print(sig.parameters["a"].default is Parameter.empty, sig.parameters["flag"].default)
print(sig.bind(1, 2, 3, extra=4).arguments)
print(inspect.signature(wrapper, follow_wrapped=False).parameters["values"].kind.name)
original.__defaults__ = (8,)
print(str(Signature.from_callable(wrapper)))


print(inspect.signature(C.method), inspect.signature(C().method))
wrapper.__signature__ = Signature()
print(inspect.signature(wrapper))
wrapper.__signature__ = None
print(tuple(inspect.signature(wrapper).parameters))
del wrapper.__signature__
wrapper.__wrapped__ = wrapper
try:
    inspect.signature(wrapper)
except ValueError:
    print("wrapper loop")
try:
    sig.bind()
except TypeError as exc:
    print(type(exc).__name__)

try:
    sig.parameters["other"] = sig.parameters["a"]
except TypeError:
    print("immutable parameters")
try:
    Parameter("not a name", Parameter.KEYWORD_ONLY)
except ValueError:
    print("invalid name")

pair.__defaults__ = (1, 2, 3)
print(inspect.signature(pair))
published = Published()
print(inspect.signature(published), published.reads)
print(Parameter("from", Parameter.POSITIONAL_ONLY))
try:
    Parameter("from", Parameter.KEYWORD_ONLY)
except ValueError:
    print("keyword rejected")

print(Parameter(InheritedName("valid"), Parameter.POSITIONAL_ONLY).name == "valid")
try:
    Parameter(DeniedName("valid"), Parameter.POSITIONAL_ONLY)
except ValueError:
    print("identifier override rejected")

assert omitted() and not omitted(None) and not omitted(object())
assert type(inspect._sentinel) is object

assert inspect.ismodule(inspect)
assert not inspect.ismodule(inspect.Parameter)


class ConstructorBase:
    def __init__(self, count: int = 2, *, flag=False) -> None: pass


class ConstructorChild(ConstructorBase):
    pass


class NewFirst(ConstructorBase):
    def __new__(cls, /, name: str): pass


class DefaultInit(ConstructorBase):
    __init__ = object.__init__


print(inspect.signature(ConstructorChild), inspect.signature(NewFirst), inspect.signature(DefaultInit))
