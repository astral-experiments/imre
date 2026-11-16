class Word:
    def __init__(self, text: str) -> None:
        self.text = text

    def __str__(self) -> str:
        print("str", self.text)
        if self.text == "stop":
            raise StopIteration
        return self.text


def main() -> None:
    words = [Word("first"), Word("stop"), Word("third")]
    print(list(map(str, words)))
    for word in map(str, words):
        print("yield", word)
    print("loop done")


main()
