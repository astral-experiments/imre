from copy import copy as snapshot


class Token:
    label: str

    def __init__(self, label: str) -> None:
        self.label = label

    def __copy__(self) -> str:
        return self.label


print(snapshot(Token("reachable via copy")))
