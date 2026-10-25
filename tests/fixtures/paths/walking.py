from itertools import cycle, islice
from pathlib import Path


root = Path('walktree')
root.mkdir()
for name in ['a', 'b', 'c']:
    (root / name).mkdir()
(root / 'a' / 'leaf').mkdir()
(root / 'b' / 'file').write_bytes(b'x')
for down in [True, False]:
    for here, dirs, files in root.walk(down, None, False):
        print(str(here))
        print('|'.join(sorted(dirs)))
        print('|'.join(sorted(files)))

# Mutate after next(): the generator must consult the very same list.
walk = root.walk()
first, pending, files = next(walk)
while pending:
    pending.pop()
pending.append('c')
pending.append('missing')
pending.append('b')
pending.append('b')
print(str(first))
print('|'.join(files))
for here, dirs, files in walk:
    print(str(here))
    print('|'.join(dirs))
    print('|'.join(files))
print('|'.join(pending))

later = Path('later').walk()
Path('later').mkdir()
print(str(next(later)[0]))
print(len(list(Path('missing').walk())))
print(len(list((root / 'b' / 'file').walk())))
bad = Path('bad\0dir').walk()
try:
    next(bad)
except ValueError as error:
    print(str(error))
print(len(list(bad)))

dot = Path('.').walk()
here, dirs, files = next(dot)
print(str(here))
while dirs:
    dirs.pop()
dirs.append('walktree/a')
print(str(next(dot)[0]))

# An islice must expose the walk's live directory list; cycle then retains
# that same row after the slice and the walk have advanced independently.
source = root.walk()
one = cycle(islice(source, 1))
held = next(one)
while held[1]:
    held[1].pop()
held[1].append('b')
print(str(next(source)[0]))
print('|'.join(next(one)[1]))
print(len(list(source)))
