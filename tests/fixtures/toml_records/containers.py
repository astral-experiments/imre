from dataclasses import dataclass
from relar import toml


@dataclass(slots=True)
class Group:
    name: str
    href: str


groups = toml.decode('''
[zebra]
name = "last alphabetically, first in source"
href = "https://example.com/z"
[alpha]
name = "second"
href = "https://example.com/a"
''', dict[str, Group])
for key, group in groups.items():
    print(key)
    print(group.name)
    print(group.href)
print(repr(toml.decode('b=[1,2]\na=[]\nc=[123456789012345678901234567890]', dict[str, list[int]])))
print(repr(toml.decode('''
small_min = -4611686018427387904
small_max = 4611686018427387903
signed_min = -9223372036854775808
signed_max = 9223372036854775807
unsigned = 18446744073709551615
hex = 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff
negative_zero = -0
positive_zero = +0
''', dict[str, int])))
print(repr(toml.decode('outer.inner.first="λ"\nouter.inner.second="x"', dict[str, dict[str, dict[str, str]]])))
print(repr(toml.decode('"quoted.key"={"nested space"=false}\na={truth=true}', dict[str, dict[str, bool]])))
print(repr(toml.decode('list=[{name="one",href="1"},{name="two",href="2"}]', dict[str, list[Group]])))
print(repr(toml.decode('numbers=[1.5,2.5,-0.0]', dict[str, list[float]])))
print(repr(toml.decode('flags=[true,false]', dict[str, list[bool]])))
print(repr(toml.decode('notes=["first","second"]', dict[str, list[str | None]])))
print(repr(toml.decode('', dict[str, int])))

# TOML 1.1 permits multiline inline tables, a trailing comma, and new escapes.
print(repr(toml.decode('''record = {
    name = "line",
    href = "https://example.com",
}
''', dict[str, Group])))
print(toml.decode('value="\\x41\\e"', dict[str, str])["value"].encode())
