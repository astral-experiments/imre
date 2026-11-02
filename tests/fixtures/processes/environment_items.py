import os

os.environ.clear()
os.environ["a"] = "first"
os.environ["b"] = "old"
for key, value in os.environ.items():
    print(key)
    print(value)
    if key == "a":
        os.environ["b"] = "new"
        os.environ["c"] = "added"

try:
    for key, value in os.environ.items():
        if key == "a":
            del os.environ["b"]
except KeyError as exc:
    print(str(exc))
else:
    raise AssertionError("expected a missing captured key")

# An early exit must not decode a later value or key.
os.environb[b"later"] = b"\xff"
os.environb[b"\xffkey"] = b"\xfe"
for key, value in os.environ.items():
    print(value)
    break

for raw_key, raw_value in os.environb.items():
    print(raw_key)
    print(raw_value)
    if raw_key == b"a":
        os.environb[b"later"] = b"\xfe\xff"
        os.environb[b"extra"] = b"ignored"
try:
    for raw_key, raw_value in os.environb.items():
        if raw_key == b"a":
            del os.environb[b"\xffkey"]
except KeyError as exc:
    print(str(exc))
else:
    raise AssertionError("expected a missing captured byte key")
