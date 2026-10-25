def parsed(text: str, known: bool, empty: bool) -> int | None:
    if not known:
        try:
            value: int | None = int(text)
        except ValueError:
            if empty:
                value = None
    else:
        value = None
    return value


def rebind(text: str) -> int:
    value = 9
    try:
        value: int = int(text)
    except ValueError:
        pass
    return value


def show(text: str, known: bool, empty: bool) -> None:
    try:
        result = parsed(text, known, empty)
        if result is None:
            print("None")
        else:
            print(result)
    except UnboundLocalError as e:
        print(str(e))


show("12", False, False)
show("bad", False, False)
show("bad", True, False)
show("bad", False, True)
print(rebind("x"))
try:
    parsed("bad", False, False)
except NameError as e:
    print(str(e))
