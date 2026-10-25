from contextvars import Context, ContextVar, Token
import errno


def attribute(choice: int) -> str:
    return ["EACCES", "errorcode"][choice]


untyped = ContextVar("untyped")
print(str(untyped).startswith("<ContextVar name='untyped' at 0x"))
print(repr(untyped).startswith("<ContextVar name='untyped' at 0x"))
untyped.set(getattr(errno, attribute(1)))
previous = untyped.set(None)
print(bool(previous.old_value))
untyped.set(getattr(errno, attribute(0)))
previous = untyped.set(None)
print(previous.old_value == errno.EACCES)
v: ContextVar[int] = ContextVar("v", default=5)
t = v.set(3)
print(repr(v).startswith("<ContextVar name='v' default=5 at 0x"))
print(repr(t).startswith("<Token var="))
print(repr(Context()).startswith("<_contextvars.Context object at 0x"))
print(repr(Token.MISSING))
v.reset(t)
print(repr(t).startswith("<Token used var="))
