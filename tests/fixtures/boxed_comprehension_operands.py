"""Compound operands retain temporaries across nested suspensions."""


class Reply:
    def __init__(self, label, value):
        self.label, self.value = label, value

    def __await__(self):
        yield self.label
        return self.value


async def collect():
    last = 0
    mapping = {
        "subscript": [(await Reply("list", [5, 6]))[0] for _ in [0]],
        "slice": [(await Reply("sequence", [3, 4, 5]))[
            await Reply("start", 1):await Reply("end", 3)] for _ in [0]],
        "set": {await Reply("one", 1), await Reply("two", 2)},
        **await Reply("mapping", {"extra": 8}),
        "named": [(last := await Reply("bound", 9)) for _ in [0]],
    }
    print("operands", mapping["subscript"], mapping["slice"], sorted(mapping["set"]),
          mapping["extra"], mapping["named"], last)

    targets = [0]
    result = [targets[0] for targets[await Reply("assign index", 0)] in [5, 6]]
    print("async assignment", result, targets)

    async def pairs():
        yield (7, 8)

    result = [(a, targets[0])
              async for a, targets[await Reply("nested index", 0)] in pairs()]
    print("async unpack assignment", result, targets)


def current():
    run = collect()
    while True:
        try:
            print("operand await", run.send(None))
        except StopIteration:
            break
