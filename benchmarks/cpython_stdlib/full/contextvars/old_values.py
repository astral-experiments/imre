from contextvars import Context, ContextVar, Token, copy_context

dynamic = ContextVar("no default")
first = dynamic.set(None)
second = dynamic.set(42)
third = dynamic.set("next")
print(first.old_value is first.MISSING)
print(second.old_value is None)
print(third.old_value == 42)
print(bool(first.old_value))
print(bool(second.old_value))
print(str(first.old_value))
print(repr(first.old_value))
print(third.var is dynamic)
dynamic.reset(third)
print(dynamic.get() == 42)
dynamic.reset(second)
print(dynamic.get() is None)
dynamic.reset(first)
print(dynamic.get(None) is None)
explicit = dynamic.set(first.old_value)
stored = dynamic.set(None)
saved = stored.old_value
assert saved is Token.MISSING
assert stored.old_value is Token.MISSING
assert first.MISSING is stored.old_value
dynamic.reset(stored)
dynamic.reset(explicit)
optional: ContextVar[object | None] = ContextVar("optional")
optional.set(first.old_value)
previous = optional.set(None)
saved_optional = previous.old_value
assert previous.old_value is Token.MISSING
assert saved_optional is first.MISSING
assert optional.set(None).old_value is None
number: ContextVar[int] = ContextVar("count", default=1)
large = 2 ** 180 + 101
number.set(large)
copied = copy_context()
prior = number.set(2)
print(prior.old_value == large)
print(copied[number] == large)
domain = Context()
token = domain.run(number.set, 5)
print(domain.run(number.get))
print(domain.run(number.get, 20))
domain.run(number.reset, token)
print(domain.run(number.get))
print(domain.run(number.get, 20))
