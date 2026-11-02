import sys
from importlib.metadata import Distribution, FileHash, PackagePath, files
from pathlib import Path

site = Path('site')
site.mkdir()
info = site / 'records-1.dist-info'
info.mkdir()
dist = Distribution.at(info)
assert dist.files is None
sys.path.insert(0, str(site))
(site / 'name,comma.txt').write_bytes(b'hello\r\n')
record = info / 'RECORD'
record.write_text('"name,comma.txt",sha256=abc,7\nmissing.txt,,\n', encoding='utf-8')
listed = files('records')
assert len(listed) == 1
item = listed[0]
assert isinstance(item, PackagePath)
assert isinstance(item.hash, FileHash)
assert (item.hash.mode, item.hash.value, item.size) == ('sha256', 'abc', 7)
assert item.read_binary() == b'hello\r\n'
assert item.read_text() == 'hello\n'

record.write_text('"name,comma.txt",sha256=abc,invalid\n', encoding='utf-8')
try:
    dist.files
except ValueError:
    pass
else:
    raise AssertionError('invalid non-empty RECORD size was accepted')

egg = site / 'records.egg-info'
egg.mkdir()
old = Distribution.at(egg)
(egg / 'SOURCES.txt').write_text('missing.txt\nname,comma.txt\n', encoding='utf-8')
assert [str(path) for path in old.files] == ['name,comma.txt']
Path('outside.txt').write_text('external', encoding='utf-8')
(egg / 'installed-files.txt').write_text('../../outside.txt\n', encoding='utf-8')
assert [str(path) for path in old.files] == ['../outside.txt']
assert old.files[0].read_text() == 'external'
print(item, repr(item.hash), item.size)
