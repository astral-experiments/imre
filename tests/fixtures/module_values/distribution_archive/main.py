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
