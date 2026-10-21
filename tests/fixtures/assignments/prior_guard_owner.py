def show(first: list[str], second: list[str]) -> bool:
    print(first[0])
    print(second[0])
    return False


def execute() -> None:
    captured: list[str] = []
    match "source":
        case "source" if len(captured := ["first"]) == 0:
            print("wrong first guard")
        case "source" if show(captured, (captured := ["second"])):
            print("wrong second guard")
        case _:
            print(captured[0])


for index in range(20):
    execute()
