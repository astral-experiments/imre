import os
import sys
from importlib.metadata import distribution, distributions, entry_points
from pathlib import Path

archive = 'installed.whl'
Path(archive).write_bytes(Path(__file__).with_name('packages.whl').read_bytes())
unsupported = str(Path(__file__).with_name('unsupported.zip'))
sys.path[:0] = [__file__, unsupported, archive]
dist = distribution('demo.tools')
assert dist.version == '2.9'
assert dist._normalized_name == 'renamed_tools'
assert distribution('legacy')._normalized_name == 'legacy'
assert dist.read_text('METADATA') == 'Name: Renamed-Tools\nVersion: 2.9\n'
assert dist.read_text('absent') is None
assert len(list(distributions(path=[archive]))) == 2
assert entry_points(group='cli', name='run')['run'].value == 'demo:main'
print(dist.name, dist.version)

if os.name == 'posix':
    Path(archive).unlink()
    Path(archive).write_bytes(Path(__file__).with_name('replacement.whl').read_bytes())
    assert dist.version == '2.9'
    assert distribution('demo.tools').version == '3.1'

# Read the manifest from the retained distribution, even after the archive was replaced.
sys.path.remove(unsupported)
listed = dist.files
assert len(listed) == 1
old_metadata = listed[0]
assert old_metadata.dist is dist
assert str(old_metadata.locate()) == archive + '/demo_tools-7.dist-info/METADATA'
assert b'Version: 2.9\r\n' in old_metadata.read_binary()
assert 'Version: 2.9\n' in old_metadata.read_text(encoding=None)
