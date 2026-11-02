import sys
from importlib.metadata import Distribution, requires
from pathlib import Path

root = Path('reporting.egg-info')
root.mkdir()
sys.path.insert(0, '.')
dist = Distribution.at(root)
assert requires('reporting') is None
reqs = root / 'requires.txt'
reqs.write_text('', encoding='utf-8')
assert dist.requires == []
reqs.write_text(
    'base\n[docs:python_version < "3.15"]\noptional @ https://example.invalid/pkg\n',
    encoding='utf-8',
)
assert requires('reporting') == [
    'base',
    'optional @ https://example.invalid/pkg ; (python_version < "3.15") and extra == "docs"',
]
(root / 'PKG-INFO').write_text('Requires-Dist: first\nRequires-Dist: second\n', encoding='utf-8')
assert dist.requires == ['first', 'second']
print(requires('reporting'))
