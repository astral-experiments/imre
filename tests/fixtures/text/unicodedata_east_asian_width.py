import unicodedata
from unicodedata import east_asian_width


def argument(text: str) -> str:
    print(text)
    return text


print(unicodedata.unidata_version)
for char in ["x", "界", "！", "ｶ", "Ω", "\x00", "\U0001fa8a", "\U000323b0", "\U0002fffe"]:
    print(east_asian_width(char))
try:
    unicodedata.east_asian_width(argument("ab"))
except TypeError as error:
    print(str(error))
try:
    east_asian_width(None)
except TypeError as error:
    print(str(error))
try:
    east_asian_width(argument("a"), argument("b"))
except TypeError as error:
    print(str(error))
try:
    east_asian_width(chr=argument("c"))
except TypeError as error:
    print(str(error))
