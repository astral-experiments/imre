from annotationlib import Format, get_annotations, type_repr


def convert(a: int, /, b: str, *, strict: bool = True) -> str:
    return b


def quoted(value: 'int') -> 'str':
    return str(value)


def nested(values: dict[str, list[int]]) -> tuple[int, str]:
    return (1, 'a')


def main() -> None:
    annotations = get_annotations(convert)
    print(annotations)
    print(annotations['a'] is int)
    print(annotations['b'] is not int)
    print(annotations['return'].__name__)
    print(annotations['strict'].__module__)
    print(type_repr(annotations['a']))
    print(type_repr(convert))
    print(type_repr('str'))
    annotations['a'] = bytes
    print(annotations)
    print(get_annotations(convert, format=Format.FORWARDREF))
    print(get_annotations(nested, format=Format.STRING))
    print(get_annotations(quoted, format=Format.STRING))
    print(get_annotations(quoted))
    print(get_annotations(quoted, eval_str=True))
    try:
        get_annotations(convert, format=Format.VALUE_WITH_FAKE_GLOBALS)
    except ValueError as exc:
        print(str(exc))
    try:
        get_annotations(convert, format=Format.STRING, eval_str=True)
    except ValueError as exc:
        print(str(exc))


main()
