from contextvars import Context, ContextVar, Token, copy_context


class Request:
    path: str
    def __init__(self, path: str):
        self.path = path


active: ContextVar[Request] = ContextVar("request")
number: ContextVar[int] = ContextVar("same", default=17)
other: ContextVar[int] = ContextVar("same", default=19)

def replace(argument: int) -> int:
    number.set(argument)
    return -2

def value(argument: int) -> int:
    added = number.get() + argument
    number.set(added)
    return added

def fail(argument: int) -> None:
    number.set(argument)
    raise ValueError("callback")

def reset_here(t: Token[int]) -> None:
    number.reset(t)

def reenter(c: Context) -> int:
    return c.run(value, 1)

order: list[int] = []
def bound_var() -> ContextVar[int]:
    order.append(1)
    return number

def eager_argument() -> int:
    order.append(2)
    number.set(120)
    return number.get() + 1


print(number.name)
print(other.name)
print(number is not other)
print(isinstance(active, ContextVar))
print(isinstance(copy_context(), Context))
print(number.get())
print(number.get(None) is None)
print(number.get(-1))
absent = active.get(None)
print(absent is None)
try:
    active.get()
except LookupError as err:
    print("request" in str(err))
request = Request("/outer")
t = active.set(request)
print(t.var is active)
print(isinstance(t, Token))
print(t.old_value is Token.MISSING)
print(active.get() is request)
snap = copy_context()
print(len(snap))
print(active in snap)
print(number in snap)
print(snap[active] is request)
print(snap.get(number) is None)
print(snap.get(number, 23))
print(number.get(replace(28)))
ta = number.set(4)
tb = number.set(6)
number.reset(ta)
print(number.get())
number.reset(tb)
print(number.get())
try:
    number.reset(ta)
except RuntimeError as err:
    print("has already been used once" in str(err))
clean = Context()
print(bool(clean))
print(bool(snap))
print(clean.run(value, 3))
print(clean[number])
print(number.get())
try:
    clean.run(reenter, clean)
except RuntimeError as err:
    print("is already entered" in str(err))
try:
    clean.run(fail, 11)
except ValueError as err:
    print(str(err))
    print(clean[number])
    print(number.get())
keep = number.set(36)
try:
    clean.run(reset_here, keep)
except ValueError as err:
    print("was created in a different Context" in str(err))
try:
    other.reset(keep)
except ValueError as err:
    print("was created by a different ContextVar" in str(err))
number.reset(keep)
with number.set(70) as managed:
    print(managed.var is number)
    print(number.get())
print(number.get())
active.reset(t)
print(active.get(None) is None)
print(snap[active] is request)
alias: ContextVar[list[int]] = ContextVar("alias")
original = [1]
alias.set(original)
snapshot = copy_context()
original.append(2)
print(snapshot[alias] is original)
print(snapshot[alias])
alias.set([3])
print(snapshot[alias])
print(alias.get())

clean.run(bound_var().set, eager_argument())
print(order)
print(number.get())
print(clean[number])
