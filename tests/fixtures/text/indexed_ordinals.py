def visit(text: str, skip: int, stop: int) -> tuple[int, int]:
    score = 0
    index = 41
    for index in range(0, len(text), 1):
        score += ord(text[index])
        if index == skip:
            continue
        score += ord(text[index]) + index
        if index == stop:
            break
    else:
        score -= 19
    return score, index


def replace_source(text: str, replacement: str) -> int:
    score = 0
    for i in range(len(text)):
        score += ord(text[i])
        text = replacement
    return score


def replace_index(text: str) -> int:
    score = 0
    for i in range(0, len(text)):
        score += ord(text[i])
        i = 0
        score += ord(text[i])
    return score


for text in ["", "a", "é界🦊0"]:
    for end in [-1, 2]:
        print(visit(text, 1, end))
    print(replace_source(text, "x" * len(text)))
    print(replace_index(text))
