def event(log: list[str], value: str) -> str:
    log.append(value)
    if value == "!":
        raise ValueError("last argument")
    return value


log: list[str] = []
print(event(log, "a") in (event(log, "a"), event(log, "b")))
print(event(log, "c") not in (event(log, "b"), event(log, "a")))
try:
    print(event(log, "a") in (event(log, "a"), event(log, "!")))
except ValueError as e:
    print(str(e))
print(event(log, "empty") in ())
for value in log:
    print(value)
print(1000000000000000000000000000000 in (1000000000000000000000000000000, 2))
print(b"x" not in (b"x", b"y"))
print(False in (True, False))
