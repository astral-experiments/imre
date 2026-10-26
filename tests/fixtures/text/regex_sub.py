import re

_filename = re.compile(r"[^A-Za-z0-9_.-]")


def normalize(filename: str) -> str:
    return re.sub(r"^([a-z_0-9]*)-([0-9]+\.)*[0-9]+", r"\1-VERSION", filename)


def reordered(value: str) -> str:
    pattern = re.compile(r"(?P<word>[a-z]+)-([0-9]+)")
    return pattern.sub(r"\2/\g<word>", value)


print(_filename.sub("", "cv_1.0-α!"))
print(reordered("red-12 blue-34"))
print(normalize("uv_build-1.23.456.dist-info/RECORD"))
print(normalize("plain/uv-3"))
print(re.sub(r"(?P<unit>[a-z])+", r"(\g<0>,\g<unit>,\1)", "éabc/def🦀"))
print(re.sub(r"(x)?y", r"\1!", "yxyy"))
print(re.sub(r"(x)$", r"<\g<0>>", "xx\n"))
print(re.sub(r"x", "", "xéxx"))
print(re.sub(r"x", r"\\\101\040\0\n\!\t$", "x"))
