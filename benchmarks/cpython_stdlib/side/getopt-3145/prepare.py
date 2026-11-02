"""Materialize the frozen replay inputs from the pinned CPython Git object."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('cpython', type=Path)
p.add_argument('kit', type=Path)
args = p.parse_args()
here = Path(__file__).resolve().parent
kit = args.kit.resolve()
kit.mkdir(parents=True, exist_ok=False)
stage = kit / 'stage'
pin = '5607950ef232dad16d75c0cf53101d9649d89115'
paths = (
    'Lib/test/__init__.py', 'Lib/test/test_getopt.py',
    'Lib/test/support/__init__.py', 'Lib/test/support/os_helper.py',
    'Lib/test/support/i18n_helper.py', 'Lib/test/support/import_helper.py',
    'Lib/test/test_tools/__init__.py', 'Lib/test/translationdata/getopt/msgids.txt',
    'Tools/i18n/pygettext.py', 'Lib/getopt.py',
)
for rel in paths:
    data = subprocess.check_output(['git', '-C', args.cpython, 'show', pin + ':' + rel])
    dest = stage / rel
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)
assert hashlib.sha256((stage / 'Lib/getopt.py').read_bytes()).hexdigest() == 'f42eecb0e854a89b0c08d4c8c10930b49fbde0baea2bfcce9b23e797b7d10a38'
shutil.copyfile(here / 'driver.py', stage / 'Lib/driver.py')
for name in ('replay.py', 'supervisor.py'):
    shutil.copyfile(here / name, kit / name)
payload = {
    'upstream': pin,
    'supervisor_source_sha256': '7ca98c7d297bb2f82a6ebb6d7da9c44fa10102e0a95a5193fb4731442c49aef9',
    'layout': {str(f.relative_to(stage)): hashlib.sha256(f.read_bytes()).hexdigest()
               for f in sorted(stage.rglob('*')) if f.is_file()},
    'ids': ["test.test_getopt.GetoptTests.test_do_longs","test.test_getopt.GetoptTests.test_do_shorts","test.test_getopt.GetoptTests.test_getopt","test.test_getopt.GetoptTests.test_gnu_getopt","test.test_getopt.GetoptTests.test_issue4629","test.test_getopt.GetoptTests.test_long_has_args","test.test_getopt.GetoptTests.test_short_has_arg","test.test_getopt.TestTranslations.test_translations","test.test_getopt.test_libref_examples"],
}
(kit / 'manifest.json').write_text(json.dumps(payload, indent=2) + '\n')
print(kit)
