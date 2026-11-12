def discard_return(flag: bool) -> list[str]:
    try:
        return ["owned " + str(42)] if flag else ["other " + str(43)]
    finally:
        raise ValueError("discarded conditional return")


def take(value: int) -> int:
    return value


saved = discard_return
try:
    take(saved(True))
except ValueError as error:
    print(str(error))
print("after cleanup")
