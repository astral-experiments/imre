import gc
import os
import sys
from importlib.metadata import MetadataPathFinder, distribution, distributions, entry_points
from pathlib import Path
from zipfile import ZipFile
from zipimport import zipimporter


def capture_distribution(path):
    found, = distributions(name='demo-tools', path=[path])
    return sys._getframe()


def archive_is_open(path):
    for name in os.listdir('/proc/self/fd'):
        try:
            if os.readlink('/proc/self/fd/' + name) == path:
                return True
        except FileNotFoundError:
            continue
    return False


archive = 'installed.whl'
Path(archive).write_bytes(Path(__file__).with_name('packages.whl').read_bytes())
if os.path.isdir('/proc/self/fd'):
    gc.disable()
    path = os.path.abspath(archive)
    frame = capture_distribution(path)
    MetadataPathFinder.invalidate_caches()
    gc.collect()
    assert archive_is_open(path)
    frame.clear()
    assert not archive_is_open(path)
    gc.enable()

unsupported = str(Path(__file__).with_name('unsupported.zip'))
assert zipimporter(unsupported).get_data('unsupported') == b''
try:
    ZipFile(unsupported)
except NotImplementedError as error:
    assert str(error) == 'zip file version 9.9'
else:
    raise AssertionError('ZipFile accepted a future extraction version')
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
