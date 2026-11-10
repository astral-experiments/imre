from string.templatelib import Template
async def get(x):
    return x

def as_base(template):
    assert isinstance(template, Template)
    print(sum(thunk() for thunk in template.interpolations[0].value))
    return object

async def make(enabled):
    class Chosen(
        (enabled and as_base(t"{[lambda v=await get(x): v for x in [1, 2]]}")) or object
    ):
        pass
    return Chosen

try:
    make(True).send(None)
except StopIteration as done:
    print(done.value.__name__)
