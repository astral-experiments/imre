import os
import sys
from importlib.metadata import Distribution, PackageNotFoundError, distribution, version
from pathlib import Path


for root, value in [('first', '5.1'), ('second', '7.9')]:
    os.mkdir(root)
    info = root + '/sample_pkg-0.dist-info'
    os.mkdir(info)
    with open(info + '/METADATA', 'w') as metadata_file:
        metadata_file.write('Name: Sample.Pkg\nVERSION: ' + value + '\n\nVersion: body\n')

sys.path[:0] = ['first', 'second']
dist = distribution('sAmPlE---pKg')
assert isinstance(dist, Distribution)
assert dist.name == 'Sample.Pkg'
assert dist.version == '5.1'
assert version('Sample.Pkg') == dist.version
assert dist.read_text('absent') is None
assert [d.version for d in Distribution.discover(path=['second', 'first'])] == ['7.9', '5.1']
later = Distribution.discover(name='sample-pkg')
sys.path = ['second'] + sys.path[1:]
assert next(later).version == '7.9'

info = Path('second/sample_pkg-0.dist-info')
assert Distribution.at(info).version == '7.9'
assert Distribution.at(str(info)).name == 'Sample.Pkg'

try:
    distribution('sample')
except PackageNotFoundError as error:
    assert error.name == 'sample'
else:
    raise AssertionError('matched an incomplete distribution name')

print(dist.name, dist.version)
