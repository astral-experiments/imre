# ruff: noqa: SIM905 - splitting these literals is the behavior under test


def main() -> None:
    parts = ("  first\tsecond\nthird\rfourth\vfifth\fsixth  ").split()
    for index in range(len(parts)):
        print(parts[index])
    controls = "a\x1cb\x1dc\x1ed\x1fe".split()
    for index in range(len(controls)):
        print(controls[index])
    unicode_spaces = "a\u0085b\u00a0c\u1680d\u2000e\u2001f\u2002g\u2003h\u2004i\u2005j\u2006k\u2007l\u2008m\u2009n\u200ao\u2028p\u2029q\u202fr\u205fs\u3000t".split()
    for index in range(len(unicode_spaces)):
        print(unicode_spaces[index])
    print(len("".split()))
    print(len(" \t\n\x1c\x1f\u3000".split()))
    not_spaces = "a\u200bb\ufeffc".split()
    print(len(not_spaces))
    print(not_spaces[0])
    print("left" + "right")
    print("" + "middle" + "")
    print("汉字" + "🙂")
    print("a\0" + "b")


main()
