def identity(value: int) -> int:
    return value
def run() -> None:
    values = map(identity, [1])
run()
