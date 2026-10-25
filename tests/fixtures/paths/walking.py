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
