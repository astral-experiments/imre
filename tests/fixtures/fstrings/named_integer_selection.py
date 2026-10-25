def marked(label: str) -> str:
    print(label)
    return "ok"


def explode() -> str:
    print("explode")
    raise RuntimeError("argument failure")


def selected(template: str, value: int) -> str:
    return template.format(
        number=value, good=marked("evaluated good"), ignored=marked("evaluated ignored")
    )


def main() -> None:
    large = 10**4300
    for template in ("{good}", "{good}{good}", "no fields", "{missing}", "{number!s}"):
        try:
            print(len(selected(template, large)))
        except KeyError as error:
            print(str(error))
        except ValueError as error:
            print(str(error))
    try:
        print("{number}".format(number=large, ignored=explode()))
    except RuntimeError as error:
        print(str(error))
    print(selected("{good}-{number}-{number}", 12))


main()
