import sys

class Registered:
    def __getstate__(self):
        return ()

async def get(x):
    return x

async def probe():
    values = []
    values += [lambda v=await get(x): v for x in [0]]
    return values[0]()

if sys.argv[1] == "take":
    try:
        probe().send(None)
    except StopIteration as result:
        print(result.value)
print("done")
