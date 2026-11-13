from _imre_mock import is_call_list, mark_call_list


@dataclass(repr=False)
class _Call:
    _args: list[_Arg]
    _kwargs: dict[str, _Arg]
    _text: str = field(compare=False)

    def __init__(self, args: list[_Arg], kwargs: dict[str, _Arg]) -> None:
        self._args = args
        self._kwargs = kwargs
        self._text = _format_call_signature("call", args, kwargs)

    @staticmethod
    def _represent(value: _Call | None) -> str:
        if value is None:
            return "None"
        return _format_recorded_call(value, "call")

    @staticmethod
    def _represent_list(values: list[_Call]) -> str:
        parts: list[str] = []
        for value in values:
            parts.append(_format_recorded_call(value, "call"))
        return _format_list(parts, is_call_list(values))


def _format_recorded_call(value: _Call, name: str) -> str:
    if name == "call":
        return value._text
    return name + value._text[4:]


def _new_call(args: list[_Arg], kwargs: dict[str, _Arg]) -> _Call:
    return _Call(args, kwargs)


def _new_call_list() -> list[_Call]:
    result: list[_Call] = []
    mark_call_list(result)
    return result


patch = _PatchFactory()
