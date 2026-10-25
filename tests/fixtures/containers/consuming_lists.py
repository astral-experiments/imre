def direct(stop: bool) -> str:
    words: list[str] = []
    words.append("first")
    words.append("last")
    result = ""
    for word in words:
        result += word
        if stop:
            break
    else:
        result += "!"
    return result


def paired(count: int, strict: bool) -> str:
    words: list[str] = []
    for i in range(3):
        words.append(str(i))
    result = ""
    for index, word in zip(range(count), words, strict=strict):
        result += str(index) + word
    return result


def aliased() -> str:
    words: list[str] = []
    words.append("one")
    alias = words
    for word in words:
        alias.append("two")
        break
    return "|".join(alias)


def revisited() -> str:
    words: list[str] = []
    words.append("keep")
    result = ""
    for i in range(2):
        for word in words:
            result += word
    return result


def same_input() -> str:
    words: list[str] = []
    words.append("both")
    result = ""
    for first, second in zip(words, words):
        result += first + second
    return result


def observe_after_error() -> str:
    words: list[str] = []
    words.append("recover")
    result = ""
    try:
        for word in words:
            raise ValueError(word)
    except ValueError:
        result = words[0]
    finally:
        result += words[0]
    return result


print(direct(True))
print(direct(False))
for n in [0, 2, 3, 4]:
    print(paired(n, False))
    try:
        print(paired(n, True))
    except ValueError as error:
        print(str(error))
print(aliased())
print(revisited())
print(same_input())
print(observe_after_error())
