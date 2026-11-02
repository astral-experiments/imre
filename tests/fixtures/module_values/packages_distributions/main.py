from importlib.metadata import packages_distributions
from pathlib import Path
import sys

first = Path('first')
second = Path('second')
declared = first / 'archive_name-1.dist-info'
inferred = second / 'file_list-2.dist-info'
declared.mkdir(parents=True)
inferred.mkdir(parents=True)
(declared / 'METADATA').write_text('Name: Declared-Owner\n', encoding='utf-8')
(declared / 'top_level.txt').write_text('shared_ns\tshared_ns\ndeclared.name\n', encoding='utf-8')
# A declaration takes precedence over reading even a malformed RECORD.
(declared / 'RECORD').write_text('irrelevant.py,,bad-size\n', encoding='utf-8')
(inferred / 'METADATA').write_text('Name: Inferred-Owner\n', encoding='utf-8')
(inferred / 'top_level.txt').write_text('\n', encoding='utf-8')
files = ['shared_ns/one.py', 'shared_ns/two.py', 'single-module.py', 'ignored.pth']
(second / 'shared_ns').mkdir()
for name in files:
    (second / name).touch()
(inferred / 'RECORD').write_text(
    ''.join(name + ',,\n' for name in [*files, 'lost_module.py', 'file_list-2.dist-info/METADATA']),
    encoding='utf-8',
)
sys.path[:0] = [str(first), str(second)]
mapping = packages_distributions()
assert mapping['shared_ns'] == ['Declared-Owner', 'Declared-Owner', 'Inferred-Owner']
assert mapping['declared.name'] == ['Declared-Owner']
assert mapping['single-module'] == ['Inferred-Owner']
assert {'ignored.pth', 'lost_module', 'file_list-2.dist-info'}.isdisjoint(mapping)
print(mapping['shared_ns'])

(declared / 'METADATA').write_text('Name: Changed-Owner\n', encoding='utf-8')
(declared / 'top_level.txt').write_text('changed_module\n', encoding='utf-8')
updated = packages_distributions()
assert updated['changed_module'] == ['Changed-Owner']
assert updated['shared_ns'] == ['Inferred-Owner']
assert 'declared.name' not in updated
