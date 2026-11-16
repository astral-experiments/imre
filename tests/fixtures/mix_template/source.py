async def get(x):
    return x
async def build():
    return t"{[lambda v=await get(x): v for x in [1, 2]]}"
