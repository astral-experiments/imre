import unicodedata
from unicodedata import is_normalized, normalize


def spelled(value: str) -> str:
    print(value)
    return value


for form in ["NFC", "NFD", "NFKC", "NFKD"]:
    for text in ["", "abc", "é", "e\u0301", "Å", "①", "각", "a\u0315\u0300",
                 "\ua7f1", "a\u0315\u1acf\u0300", "\U0001ccd6"]:
        result = unicodedata.normalize(form, text)
        print(ascii(result))
        print(is_normalized(form, text))
        print(normalize(form, result) == result)
print(normalize("bad", "") == "")
print(is_normalized("bad", ""))
try:
    normalize("bad", "text")
except ValueError as error:
    print(str(error))
try:
    normalize(spelled("NFD"), 42)
except TypeError as error:
    print(str(error))
try:
    normalize(b"NFD", spelled("text"))
except TypeError as error:
    print(str(error))
try:
    is_normalized("NFD", None)
except TypeError as error:
    print(str(error))
try:
    normalize("NFD")
except TypeError as error:
    print(str(error))
try:
    normalize("NFD", spelled("left"), spelled("right"))
except TypeError as error:
    print(str(error))
try:
    normalize(form=spelled("NFD"), unistr=spelled("text"))
except TypeError as error:
    print(str(error))
