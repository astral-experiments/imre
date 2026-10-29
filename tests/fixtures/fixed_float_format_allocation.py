import sys


def render(number: float, shape: int) -> str:
    if shape == 0:
        return f'{number:.1000f}'
    if shape == 1:
        return f'{number:+.1000f}'
    if shape == 2:
        return f'{number: .1000f}'
    if shape == 3:
        return f'{number:.1000%}'
    if shape == 4:
        return f'{number:+.1000%}'
    if shape == 5:
        return f'{number: .1000%}'
    if shape == 6:
        return f'{number:.6f}'
    if shape == 7:
        return f'{number:+.6f}'
    return f'{number:.6%}'


def prefixed(number: float, shape: int) -> str:
    return f'{number:+.6f}|{render(number, shape)}'


number = float(sys.argv[1])
shape = int(sys.argv[2])
prefix = int(sys.argv[3])
try:
    if prefix:
        result = prefixed(number, shape)
    else:
        result = render(number, shape)
    print(result)
except MemoryError:
    print('MemoryError')
if prefix:
    retry = prefixed(number, shape)
else:
    retry = render(number, shape)
print(retry)
