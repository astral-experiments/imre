import re
from re import UNICODE, VERBOSE


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
