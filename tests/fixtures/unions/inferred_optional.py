def read(flag: bool) -> str | None:
    if flag:
        return "ok"
    return None

def pull(enabled: bool, flag: bool) -> bool:
    value = None
    print("initial", value is None)
    if enabled:
        value = read(flag)
    return value is None

print(pull(False, True), pull(True, False), pull(True, True))
