from pathlib import Path, PosixPath


def check(path: Path, pattern: Path, sensitive: bool | None) -> None:
    try:
        right = path.match(path_pattern=pattern, case_sensitive=sensitive)
        print(right)
    except ValueError as error:
        print(str(error))
    print(path.full_match(case_sensitive=sensitive, pattern=pattern))


for text in ["", "/", "//", "a", "/a", "//a", "a/b", "/a/b", ".dot",
             "a\nb", "İ", "ı", "K", "ſ", "]", "雪"]:
    for pattern in ["", ".", "/", "//", "*", "?", "**", "***", "**/*", "a/**",
                    "**/a", "/**/a", "//**", "a/**/**/b", "[!a]", "[z-a]",
                    "[!z-a]", "[a-c]", "[[]", "[]]", "[a-z]", "[!I]", "[ſ]"]:
        p = PosixPath(text)
        q = Path(pattern)
        check(p, q, None)
        check(p, q, False)
        check(p, q, True)
