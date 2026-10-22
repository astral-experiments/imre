from re import Pattern, VERBOSE as verbose, compile as compile_pattern

print("initializing patterns")
WORD: Pattern[str] = compile_pattern(r"(?P<word> [a-z]+ )", flags=verbose)
PATTERNS: list[Pattern[str]] = [WORD]


def first_word(text: str):
    return WORD.search(text)
