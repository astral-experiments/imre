import re


WORD = re.compile(r"^\w$")
NONWORD = re.compile(r"^\W$")
FIELD = re.compile(r"^[.\w]+$")
NEGATED = re.compile(r"^[^\W]+$")
for text in ["a", "_", "8", "楽", "٢", "Ⅵ", "²", "℘", "‿", "́", "‌", ".", "-", "", "name.part"]:
    print(WORD.search(text) is not None)
    print(NONWORD.search(text) is not None)
    print(FIELD.search(text) is not None)
    print(NEGATED.search(text) is not None)
