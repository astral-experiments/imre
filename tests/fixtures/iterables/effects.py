class Holder:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def replace(holder: Holder) -> int:
    holder.values.append(3)
    holder.values = [99]
    return 10


def source(log: list[str]) -> list[str]:
    log.append("source")
    return ["2", "3"]


def start(log: list[str]) -> int:
    log.append("start")
    return 7


def repair(values: list[str]) -> int:
    values[0] = "7"
    values.append("8")
    return 10


def fail_start() -> int:
    print("start failed")
    raise ValueError("start error")


holder = Holder([1, 2])
old = holder.values
print(sum(holder.values, replace(holder)))
print(len(old))
print(holder.values[0])
holder = Holder([1, 2])
print(sum(map(int, holder.values), replace(holder)))
print(holder.values[0])
log: list[str] = []
print(sum(map(int, source(log)), start(log)))
for event in log:
    print(event)

words = ["bad"]
print(sum(map(int, words), repair(words)))
print(len(words))
try:
    print(sum(map(int, ["bad"]), fail_start()))
except ValueError as error:
    print(error)
