from _imre_mock import mark_call_list


@dataclass(repr=False)
class _Call:
    _args: tuple[object, ...]
    _kwargs: dict[str, object]

    def __repr__(self) -> str:
        return _format_recorded_call(self, "call")


def _format_recorded_call(value: _Call, name: str) -> str:
    return _format_call_signature(name, value._args, value._kwargs)


def _new_call(args: tuple[object, ...], kwargs: dict[str, object]) -> _Call:
    return _Call(args, kwargs)


def _new_call_list() -> list[_Call]:
    result: list[_Call] = []
    mark_call_list(result)
    return result


patch = _PatchFactory()
