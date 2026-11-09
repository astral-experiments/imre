import os
import sys
from importlib.metadata import Distribution, DistributionFinder, MetadataPathFinder, PackageNotFoundError, distribution, version
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
context = DistributionFinder.Context(path=['second'], realm='private')
assert context.name is None and context.realm == 'private'
assert [d.version for d in MetadataPathFinder.find_distributions(context)] == ['7.9']


class Provider(DistributionFinder):
    def find_distributions(self, context):
        assert context.realm == 'private'
        return [Distribution.at('second/sample_pkg-0.dist-info')]


provider = Provider()
sys.meta_path.append(provider)
try:
    assert [d.version for d in Distribution.discover(path=[], realm='private')] == ['7.9']
finally:
    sys.meta_path.remove(provider)

original_meta_path = sys.meta_path
finders = []
try:
    sys.meta_path = finders
    pending = Distribution.discover(context=context)
    sys.meta_path = []
    finders.append(provider)
    assert [d.version for d in pending] == ['7.9']

    sys.meta_path = None
    try:
        Distribution.discover(context=context)
    except TypeError:
        pass
    else:
        raise AssertionError('discover should acquire the finder iterator immediately')
finally:
    sys.meta_path = original_meta_path

query = DistributionFinder.Context(name='sample-pkg')
found = MetadataPathFinder.find_distributions(query)
query.name = 'absent'
later = Distribution.discover(name='sample-pkg')
sys.path = ['second'] + sys.path[1:]
assert next(found).version == '5.1'
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
