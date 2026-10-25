import re


def normalize(filename: str) -> str:
    return re.sub(r"^([a-z_0-9]*)-([0-9]+\.)*[0-9]+", r"\1-VERSION", filename)


print(normalize("uv_build-1.23.456.dist-info/RECORD"))
print(normalize("plain/uv-3"))
print(re.sub(r"(?P<unit>[a-z])+", r"(\g<0>,\g<unit>,\1)", "éabc/def🦀"))
print(re.sub(r"(x)?y", r"\1!", "yxyy"))
print(re.sub(r"(x)$", r"<\g<0>>", "xx\n"))
print(re.sub(r"x", "", "xéxx"))
print(re.sub(r"x", r"\\\101\040\0\n\!\t$", "x"))
