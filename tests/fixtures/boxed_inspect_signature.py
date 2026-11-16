import builtins
from collections import OrderedDict
import inspect
import operator
import sys
import warnings
from annotationlib import Format
from functools import Placeholder, partial, partialmethod, update_wrapper
from inspect import FullArgSpec, Parameter, Signature, getfullargspec


def omitted(value=inspect._sentinel):
    return value is inspect._sentinel


def original(a, /, b: int = 3, *args, flag=False, **kwargs) -> str:
    return str(a)


def annotated(value: "Element") -> "Element":
    return value


def deferred(value: MissingAnnotation) -> int:
    pass


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
    __text_signature__ = "(name, /)"

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


class JoinCall(str):
    __call__ = str.join


class BoundJoinCall:
    __call__ = ":".join


class DictFactory(dict):
    __call__ = dict.__dict__["fromkeys"]


class CustomDict(dict):
    def __init__(self, fields):
        super().__init__(fields)


class ReadOnce:
    def __get__(self, obj, owner):
        obj.reads += 1
        if obj.reads > 1:
            raise RuntimeError("descriptor read twice")
        return partial(with_options, "label")


class Handler:
    __call__ = ReadOnce()

    def __init__(self):
        self.reads = 0


class SlottedHandler:
    __slots__ = "__call__"


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


class DocumentedFactory:
    """DocumentedFactory(value, /, *, retries=2)\n--\n\nFactory protocol."""

    __text_signature__ = '(spoof)'


class InheritedFactory(DocumentedFactory):
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


class InheritedAdapter(staticmethod):
    pass


class NamedAdapter(staticmethod):
    def __init__(self, callback, *, name):
        super().__init__(callback)


class JoinMeta(type):
    def __call__(cls, *args, **kwargs):
        return ":".join(*args, **kwargs)

    __call__.__wrapped__ = ":".join


class Joined(metaclass=JoinMeta):
    def __init__(self, parts):
        pass


def documented(*args, **kwargs):
    pass


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

string_sig = inspect.signature(deferred, annotation_format=Format.STRING)
assert string_sig.parameters["value"].annotation == "MissingAnnotation"
assert string_sig.return_annotation == "int"
forward_sig = Signature.from_callable(deferred, annotation_format=Format.FORWARDREF)
assert forward_sig.parameters["value"].annotation.__forward_arg__ == "MissingAnnotation"
assert forward_sig.return_annotation is int

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


assert str(inspect.signature(CallableInstance)) == "()"
target = CallableInstance()
target.__call__ = lambda shadow: None
assert str(inspect.signature(target)) == "(name, /)"
target.__text_signature__ = "(value, /, *, width=80)"
assert str(inspect.signature(target)) == "(value, /, *, width=80)"
target.__text_signature__ = ""
assert str(inspect.signature(target)) == "(item, /, *, count=1)"
assert str(inspect.signature(StaticCall())) == "(first, second)"
assert str(inspect.signature(ClassCall())) == "(item)"
for join in (JoinCall(":"), BoundJoinCall()):
    sig = inspect.signature(join)
    assert str(sig) == "(iterable, /)"
    assert join(*sig.bind(["a", "bc"]).args) == "a:bc"

factory = DictFactory()
assert inspect.signature(factory) == inspect.signature(dict.fromkeys)
assert type(factory(["key"])) is DictFactory
assert str(inspect.signature(CustomDict)) == "(fields)"
for cls in (dict, DictFactory):
    try:
        inspect.signature(cls)
    except ValueError:
        pass
    else:
        raise AssertionError("native dict constructor acquired a signature")

for factory, arg, constructor in (
    (operator.attrgetter, "real", "(attr, /, *attrs)"),
    (operator.itemgetter, 0, "(item, /, *items)"),
    (operator.methodcaller, "upper", "(name, /, *args, **kwargs)"),
):
    operation = factory(arg)
    assert str(inspect.signature(factory)) == constructor
    assert str(inspect.signature(operation)) == "(obj, /)"
    assert factory.__text_signature__ == constructor
    assert operation.__text_signature__ == "(obj, /)"
    assert str(inspect.signature(operation.__call__)) == "(*args, **kwargs)"

assert operator.attrgetter.__call__(operator.attrgetter("__name__"), original) == "original"
assert operator.itemgetter.__call__(operator.itemgetter(0), [8]) == 8
assert operator.methodcaller.__call__(operator.methodcaller("upper"), "ok") == "OK"


class methodcaller:
    __module__ = "operator"

    def __call__(self, value):
        return value


lookalike = methodcaller()
assert str(inspect.signature(lookalike)) == "(value)"
try:
    operator.methodcaller.__call__(lookalike, "ok")
except TypeError:
    pass
else:
    raise AssertionError("native wrapper accepted a Python lookalike")

power = (2).__pow__
assert str(inspect.signature(power)) == "(value, mod=None, /)"
assert power(3, 5) == 3
assert int.__pow__(2, 1.5) is NotImplemented

handler = Handler()
handler.__call__ = lambda wrong: None
assert inspect.signature(handler) == inspect.signature(partial(with_options, "label"))
assert handler.reads == 1
try:
    inspect.signature(handler)
except RuntimeError as error:
    assert str(error) == "descriptor read twice"
else:
    raise AssertionError("descriptor exception was lost")
assert handler.reads == 2

slotted = SlottedHandler()
slotted.__call__ = lambda value: value
assert str(inspect.signature(slotted)) == "(value)"
assert slotted(7) == 7

update_wrapper(wrapper, original)
spec = inspect.getfullargspec(original)
assert type(spec) is FullArgSpec
assert spec == FullArgSpec(["a", "b"], "args", "kwargs", (3,), ["flag"], {"flag": False},
                           {"b": int, "return": str})
assert getfullargspec(wrapper).varargs == "values"
assert getfullargspec(C().method).args == ["self", "value"]
receiver = C()
assert inspect.getcallargs(receiver.method) == {"self": receiver, "value": b"x"}
assert inspect.getcallargs(original, 1, 2, 3, extra=4) == {
    "a": 1, "b": 2, "args": (3,), "flag": False, "kwargs": {"extra": 4},
}
try:
    inspect.getcallargs(receiver.method, b"x", value=b"y")
except TypeError as exc:
    assert str(exc) == "method() got multiple values for argument 'value'"
else:
    raise AssertionError("getcallargs accepted two values for the same parameter")
assert getfullargspec(partial(C().method, b"x")).args == ["value"]
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


DocumentedFactory.__doc__ = 'new documentation'
assert DocumentedFactory.__text_signature__ == '(value, /, *, retries=2)'
assert InheritedFactory.__text_signature__ is None
bound = inspect.signature(InheritedFactory).bind("payload", retries=3)
assert bound.arguments == {"value": "payload", "retries": 3}
assert inspect.getattr_static(type, "__text_signature__").__get__(tuple, type) == tuple.__text_signature__
assert str(inspect.signature(type("Entries", (tuple,), {}))) == "(iterable=(), /)"

print(inspect.signature(ConstructorChild), inspect.signature(NewFirst), inspect.signature(DefaultInit))


assert inspect.signature(DeprecatedChild) == inspect.signature(ConstructorBase)
assert str(inspect.signature(DeprecatedChild, follow_wrapped=False)) == "(*args, **kwargs)"
DeprecatedChild.__new__.__signature__ = Signature([
    Parameter("cls", Parameter.POSITIONAL_ONLY), Parameter("published", Parameter.KEYWORD_ONLY),
])
assert str(inspect.signature(DeprecatedChild)) == "(*, published)"


for adapter in (classmethod, staticmethod):
    assert adapter.__text_signature__ == "(function, /)"
    assert str(inspect.signature(adapter)) == "(function, /)"
assert str(inspect.signature(InheritedAdapter)) == "(function, /)"
assert str(inspect.signature(NamedAdapter)) == "(callback, *, name)"

assert str(inspect.signature(DescriptorInit)) == "(selected)"

assert str(inspect.signature(Joined)) == "(parts)"
assert str(inspect.signature(Joined, follow_wrapped=False)) == "(*args, **kwargs)"
JoinMeta.__call__.__signature__ = Signature([
    Parameter("cls", Parameter.POSITIONAL_ONLY), Parameter("items", Parameter.KEYWORD_ONLY),
])
assert str(inspect.signature(Joined)) == "(*, items)"
JoinMeta.__call__ = ":".join
assert str(inspect.signature(Joined, follow_wrapped=False)) == "(parts)"


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
assert getfullargspec(bound) == getfullargspec(raw) == FullArgSpec(
    ["self", "instance", "owner"], None, None, (None,), [], None, {}
)
assert sig.parameters["owner"].default is None
assert sig.parameters["instance"].kind is Parameter.POSITIONAL_ONLY

assert str(inspect.signature(object.__init__)) == "(self, /, *args, **kwargs)"
assert str(inspect.signature(object().__init__)) == "(*args, **kwargs)"
assert str(inspect.signature(list.append)) == "(self, object, /)"
assert str(inspect.signature([].append)) == "(object, /)"

assert str(inspect.signature(object.__new__, follow_wrapped=False)) == "(*args, **kwargs)"
assert getfullargspec(object.__new__).args == ["type"]
assert str(inspect.signature("|".join)) == "(iterable, /)"
assert str(inspect.signature(partial("|".join, ("a", "b")))) == "()"

original_builtins = sys.modules.pop("builtins")
try:
    import builtins as fresh_builtins
    for module in (builtins, fresh_builtins):
        assert module.sorted.__self__ is original_builtins
        assert "sorted.__self__" not in vars(module)
        assert getattr(module, "sorted.__self__", None) is None
finally:
    sys.modules["builtins"] = original_builtins
assert str(inspect.signature(sorted)) == "(iterable, /, *, key=None, reverse=False)"
assert inspect.getfullargspec(sorted).args == ["iterable"]

try:
    inspect.signature(min)
except ValueError as exc:
    assert str(exc) == f"no signature found for builtin {min!r}"
else:
    raise AssertionError("min has no text signature")
try:
    inspect.signature(bytes.count)
except ValueError as exc:
    assert str(exc) == f"{bytes.count!r} builtin has invalid signature"
else:
    raise AssertionError("invalid bytes.count text signature was accepted")

documented.__self__ = object()
documented.__text_signature__ = "($self, /, value, *, retries=1)"
assert str(inspect.signature(documented, annotation_format=Format.STRING)) == "(value, *, retries=1)"
assert inspect.getfullargspec(documented).args == ["self", "value"]

documented.__text_signature__ = "(self, 123)"
try:
    inspect.signature(documented)
except ValueError:
    pass
else:
    raise AssertionError("invalid function text signature was accepted")
