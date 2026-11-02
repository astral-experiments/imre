from importlib.metadata import Distribution
from json import JSONDecodeError
from pathlib import Path
from types import SimpleNamespace

root = Path('sample-1.0.dist-info')
root.mkdir()
dist = Distribution.at(root)
assert dist.origin is None

record = root / 'direct_url.json'
record.write_text(
    '{"url":"file:///sample.whl","archive_info":{"hashes":{"sha256":"abc"}}}',
    encoding='utf-8',
)
origin = dist.origin
assert type(origin) is SimpleNamespace
assert origin.url == 'file:///sample.whl'
assert type(origin.archive_info.hashes) is SimpleNamespace
assert origin.archive_info.hashes.sha256 == 'abc'

record.write_text('{"url":"file:///sample.tar.gz"}', encoding='utf-8')
assert not dist.origin.url.endswith('.whl')
for invalid in ('', '{'):
    record.write_text(invalid, encoding='utf-8')
    try:
        dist.origin
    except JSONDecodeError:
        pass
    else:
        raise AssertionError('invalid direct_url.json was accepted')
print(origin.url)
