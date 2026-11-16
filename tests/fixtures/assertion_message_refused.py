from string.templatelib import Template
async def get(x):
    return x
async def probe(enabled):
    assert False, t"{[lambda v=await get(x): v for x in [1, 2]]}"
    return 1
try:
    probe(True).send(None)
except StopIteration as done:
    print(done.value)
