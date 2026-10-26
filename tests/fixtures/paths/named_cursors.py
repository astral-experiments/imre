from pathlib import Path


def root() -> Path:
    print('acquire directory')
    return Path('named-cursors')


base = Path('named-cursors')
base.mkdir()
(base / 'child').mkdir()
(base / 'child' / 'deep').mkdir()
(base / 'entry').write_text('file')

walk = root().walk()
first, dirs, files = next(walk)
print(first.name)
print([r.name for r, _, _ in walk])
print([r.name for r, _, _ in walk])
entries = root().iterdir()
print(sorted([p.name for p in entries]))
print([p.name for p in entries])
print([r.name for r, _, _ in root().walk()])
