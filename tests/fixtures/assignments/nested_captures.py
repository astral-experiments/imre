def execute() -> None:
    source = [["initial"]]
    saved: list[str] = []
    match source:
        case [saved] if (
            len([[(saved := [str(inner)]) for inner in range(2)] for outer in range(2)])
            == 0
        ):
            print("wrong")
        case [later]:
            print(saved[0])
            print(later[0])
            copied = [saved for saved in later]
            print(copied[0])
            print(saved[0])
    optional: str | None = None
    match ["initial"]:
        case [optional] if optional is not None:
            results = [
                (optional := None)
                for outer in range(1)
                if optional is not None
                for inner in range(2)
                if optional is not None
            ]
            print(len(results))
            print(optional is None)
    binding: str | None = "bound"
    match binding:
        case "bound" if len([(binding := None) for index in range(1)]) == 0:
            print("wrong")
        case "bound":
            print(binding is None)
        case _:
            print("wrong subject")


execute()
