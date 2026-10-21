"""String loops capture the original Unicode string and own yielded characters."""


class Source:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


def replace_start(source: Source) -> int:
    source.text = "replacement"
    return 1000000000000000000000000000000


def keep(source: Source, character: str) -> bool:
    source.text = "replaced during comprehension"
    return character != "x"


def first_kept(text: str) -> str:
    for character in text:
        if character == "x":
            continue
        return character
    return "empty"


def main() -> None:
    for text in ["", "a", "é", "\x00", "aé🌍\x00e\u0301"]:
        print(len(text))
        for index in range(-len(text), len(text)):
            print(text[index])
        for character in text:
            print(character)
        for index, character in enumerate(text, 1000000000000000000000000000000):
            print(index)
            print(character)
        characters = [character for character in text]
        for character in characters:
            print(character)
        indexed = {character: index for index, character in enumerate(text)}
        for character, index in indexed.items():
            print(character)
            print(index)
        unique = {character for character in text}
        print(len(unique))

    captured = "aé🌍"
    for character in captured:
        captured = "replacement"
        print(character)
        character = "rebound target"
    print(captured)
    source = Source("aé🌍")
    for index, character in enumerate(source.text, replace_start(source)):
        print(index)
        print(character)
    print(source.text)
    source.text = "axé🌍x"
    filtered = [character for character in source.text if keep(source, character)]
    for character in filtered:
        print(character)
    print(source.text)
    print(first_kept("xx🌍"))
    print(first_kept("xxx"))
    for character in "é🌍z":
        print(character)
        break
    nested = [left + right for left in "aé" for right in "🌍z"]
    for pair in nested:
        print(pair)


main()
