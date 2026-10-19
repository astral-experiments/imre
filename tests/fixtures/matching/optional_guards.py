def choose(flag: bool | None) -> bool:
    match 1:
        case _ if flag:
            return flag
        case _:
            return False


print(choose(None))
print(choose(False))
print(choose(True))
