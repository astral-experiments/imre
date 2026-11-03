def source(log: list[int], value: str) -> str:
    log.append(1)
    return value

def index(log: list[int], at: int) -> int:
    log.append(2)
    return at

def check(value: str, at: int) -> None:
    log: list[int] = []
    try:
        print("x" == source(log, value)[index(log, at)])
        print(source(log, value)[index(log, at)] != "\0")
        print(source(log, value)[index(log, at)] == "é")
    except IndexError as error:
        print(str(error))
    print(log)

check("x", 0)
check("漢éx", -2)
check("é", 0)
check("\0", 0)
check("", 0)
check("x", -2)
check("x", 9223372036854775809)
