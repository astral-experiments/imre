from email.message import Message
from importlib.metadata import Distribution, metadata
from pathlib import Path
import sys

root = Path('reporting-99.dist-info')
root.mkdir()
(root / 'METADATA').write_text(
    'nAmE: Reporting\nVersion: 2.7\nClassifier: first\ncLaSsIfIeR: second\n'
    'Keywords: read write\nSummary: first line\n        second line\n\n'
    'Description: body text\n',
    encoding='utf-8',
)
sys.path.insert(0, '.')
md = metadata('reporting')
assert isinstance(md, Message)
assert Distribution.at(root).name == md['Name'] == 'Reporting'
assert md.get_all('Classifier') == ['first', 'second']
assert md.get('classifier') == 'first'
assert md.get_all('missing') is None
assert md.get('missing') is None
assert md['summary'] == 'first line\nsecond line'
assert md['description'] == 'Description: body text\n'
assert 'VERSION' in md
assert len(md) == 7
assert list(md)[:4] == ['nAmE', 'Version', 'Classifier', 'cLaSsIfIeR']
assert md.json['classifier'] == ['first', 'second']
assert md.json['keywords'] == ['read', 'write']
# Metadata headers remain readable when an installer also writes MIME fields.
(root / 'METADATA').write_text(
    'Name: Reporting\nVersion: 3.4\nContent-Type: message/rfc822\n\nsubject: note\n',
    encoding='utf-8',
)
assert Distribution.at(root).version == '3.4'
print(md['Name'], md['Version'])
