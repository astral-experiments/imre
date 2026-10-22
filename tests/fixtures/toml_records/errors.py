from dataclasses import dataclass
from relar.toml import decode


@dataclass
class Item:
    value: int = 7


@dataclass
class Required:
    value: int | None


@dataclass
class Owned:
    name: str
    counts: list[int]


try:
    missing = decode('', Required)
    assert False, "unexpected missing optional field"
except ValueError:
    print("optional annotation still requires its field")


for text in [
    'value=true', 'value=1.0', 'value="1"', 'value=[]', 'value={}',
    'value=1979-05-27', 'value=07:32:00', 'value=1979-05-27T07:32:00Z',
    'value=null', 'value=01', 'value=1_', 'value=0x_G',
    'value=1\nvalue=2', 'ignored=1\nignored=2', '[ignored]\nx=1\n[ignored]\ny=2',
    'ignored=[1,2', 'ignored="\\uD800"', 'ignored="\\q"',
    'ignored=2023-02-29', 'ignored=0000-01-01', 'ignored=23:59:60',
    'ignored={a=1,a=2}', 'ignored={a=1}\nignored.b=2',
]:
    try:
        value = decode(text, Item)
        assert False, "unexpected integer or invalid document"
    except ValueError:
        print("invalid integer or document")

for text in ['value=1', 'value=true', 'value="1.0"', 'value=1979-05-27']:
    try:
        float_values = decode(text, dict[str, float])
        assert False, "unexpected float"
    except ValueError:
        print("invalid float")

for text in ['value=1979-05-27', 'value=07:32:00', 'value=1979-05-27T07:32:00Z']:
    try:
        datetime_records = decode(text, dict[str, Item])
        assert False, "unexpected datetime record"
    except ValueError:
        print("datetime is not a record")
    try:
        dictionary = decode(text, dict[str, dict[str, str]])
        assert False, "unexpected datetime dictionary"
    except ValueError:
        print("datetime is not a dictionary")

print(decode('ignored=1979-05-27\nmore=07:32\nlast=1979-05-27T07:32Z', Item).value)
for text in ['values=[1,true]', 'values=[1,1.0]', 'values=[1,"2"]']:
    try:
        values = decode(text, dict[str, list[int]])
        assert False, "unexpected mixed list"
    except ValueError:
        print("invalid mixed list")

for iteration in range(100):
    try:
        values = decode('first=[1,2,3]\nsecond=[4,5,"bad"]', dict[str, list[int]])
        assert False, "unexpected recovery"
    except ValueError:
        pass
    try:
        record = decode('name="owned λ"\ncounts=[1,2,"bad"]', Owned)
        assert False, "unexpected partial record"
    except ValueError:
        pass
print(repr(decode('values=[7,8,9]', dict[str, list[int]])))
print(decode('name="recovered"\ncounts=[3,4]', Owned))
