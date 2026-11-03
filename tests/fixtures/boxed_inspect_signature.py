from collections import OrderedDict
import inspect
import warnings
from annotationlib import Format
from functools import Placeholder, partial, partialmethod, update_wrapper
from inspect import Parameter, Signature


def omitted(value=inspect._sentinel):
    return value is inspect._sentinel


def original(a, /, b: int = 3, *args, flag=False, **kwargs) -> str:
    return str(a)


def annotated(value: "Element") -> "Element":
    return value


def with_options(a, b, *args, flag=False):
    return a, b, args, flag


class PartialMethods:
    def method(receiver, value, option):
        pass

    with_hole = partialmethod(method, Placeholder, 2)
    variadic = partialmethod(lambda *args: None, 1)


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


class FunctionProxy:
    def __init__(self, func):
        self.__name__ = func.__name__
        self.__code__ = func.__code__
        self.__defaults__ = func.__defaults__
        self.__kwdefaults__ = func.__kwdefaults__
        self.__annotations__ = func.__annotations__

    def __call__(self, *args, **kwargs):
        pass


class StaticCall:
    __call__ = staticmethod(lambda first, second: None)


class ClassCall:
    @classmethod
    def __call__(cls, item):
        return item


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


class ConstructorBase:
    def __init__(self, count: int = 2, *, flag=False) -> None: pass


class ConstructorChild(ConstructorBase):
    pass


class NewFirst(ConstructorBase):
    def __new__(cls, /, name: str): pass


class DefaultInit(ConstructorBase):
    __init__ = object.__init__


@warnings.deprecated("old constructor")
class DeprecatedChild(ConstructorBase):
    pass


class InitDescriptor:
    __wrapped__ = object.__init__

    def __get__(self, instance, owner):
        return lambda selected: None


class DescriptorInit:
    __init__ = InitDescriptor()


proxy = FunctionProxy(original)
assert inspect.signature(proxy) == inspect.signature(original)
proxy.__defaults__ = (5,)
proxy.__kwdefaults__ = {"flag": True}
proxy.__annotations__ = {"return": bytes}
proxy_sig = inspect.signature(proxy)
assert proxy_sig.parameters["b"].default == 5
assert proxy_sig.parameters["flag"].default is True
assert proxy_sig.return_annotation is bytes
proxy.__code__ = compile("1 + 1", "<expr>", "eval")
proxy.__annotations__ = {}
assert str(inspect.signature(proxy)) == "()"
del proxy.__defaults__
assert str(inspect.signature(proxy)) == "(*args, **kwargs)"

assert inspect.signature(annotated).parameters["value"].annotation == "Element"
resolved = Signature.from_callable(
    annotated, globals={"Element": str}, locals={"Element": int}, eval_str=True)
assert resolved.parameters["value"].annotation is int
assert resolved.return_annotation is int
try:
    inspect.signature(annotated, globals={}, locals={}, eval_str=True)
except NameError:
    pass
else:
    raise AssertionError("missing annotation name was silently accepted")
try:
    inspect.signature(annotated, eval_str=True, annotation_format=Format.STRING)
except ValueError:
    pass
else:
    raise AssertionError("string formatting with eval_str was silently accepted")


target = CallableInstance()
target.__call__ = lambda shadow: None
assert str(inspect.signature(target)) == "(item, /, *, count=1)"
assert str(inspect.signature(StaticCall())) == "(first, second)"
assert str(inspect.signature(ClassCall())) == "(item)"

update_wrapper(wrapper, original)
sig = Signature.from_callable(wrapper)
assert type(sig.parameters.copy()) is OrderedDict
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
for name in ("not a name", "x\ud800"):
    try:
        Parameter(name, Parameter.KEYWORD_ONLY)
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


print(inspect.signature(ConstructorChild), inspect.signature(NewFirst), inspect.signature(DefaultInit))


assert inspect.signature(DeprecatedChild) == inspect.signature(ConstructorBase)
assert str(inspect.signature(DeprecatedChild, follow_wrapped=False)) == "(*args, **kwargs)"
DeprecatedChild.__new__.__signature__ = Signature([
    Parameter("cls", Parameter.POSITIONAL_ONLY), Parameter("published", Parameter.KEYWORD_ONLY),
])
assert str(inspect.signature(DeprecatedChild)) == "(*, published)"


assert str(inspect.signature(DescriptorInit)) == "(selected)"


assert str(inspect.signature(partial(with_options, 1))) == "(b, *args, flag=False)"
assert str(inspect.signature(partial(with_options, b=2))) == "(a, *, b=2, flag=False)"
assert str(inspect.signature(partial(with_options, Placeholder, 2))) == "(a, /, *args, flag=False)"
assert str(inspect.signature(PartialMethods.with_hole)) == "(receiver, value, /)"
assert str(inspect.signature(PartialMethods().with_hole)) == "(value, /)"
assert str(inspect.signature(PartialMethods.variadic)) == "(*args)"


bound = object.__init__.__get__
raw = type(object.__init__).__get__
assert bound.__text_signature__ == raw.__text_signature__ == "($self, instance, owner=None, /)"
assert str(Signature.from_callable(raw)) == "(self, instance, owner=None, /)"
sig = inspect.signature(bound)
assert str(sig) == "(instance, owner=None, /)"
assert sig.parameters["owner"].default is None
assert sig.parameters["instance"].kind is Parameter.POSITIONAL_ONLY

assert str(inspect.signature(object.__init__)) == "(self, /, *args, **kwargs)"
assert str(inspect.signature(object().__init__)) == "(*args, **kwargs)"
