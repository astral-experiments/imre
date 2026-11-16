import re
from re import A, UNICODE, VERBOSE


pattern = re.compile(r"""
    ^ (?P<name> .+? ) - (?P<version> .*? )
    (?: - (?P<build> \d [^-]*? ))? - (?P<tags> .+? - .+? - .+? )
    \.whl $  # terminal anchor followed by layout
""", UNICODE | VERBOSE)

def show(value: str) -> None:
    found = pattern.match(value)
    if found is not None:
        print(repr(found.groups()))
    else:
        print("miss")

for value in ["n-1-py3-none-linux.whl", "é-2-٣abc-py3-none-win.whl", "a-1-b-c-d.whl\n", "broken"]:
    show(value)
print(bool(re.search(r"\d", "٣", flags=UNICODE)))


ascii_pattern = re.compile(r"^(\w+) \s (\d+)$", A | VERBOSE)
for value in ["cat 42", "café 42", "cat\u00a042", "cat ٤٢"]:
    print(bool(ascii_pattern.match(value)))
print(bool(re.search(r"\bé", "xé", flags=re.ASCII)))
print(bool(re.match(r"\D", "١", re.A)))
