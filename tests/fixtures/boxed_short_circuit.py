class Probe:
    def __init__(self, answers, log):
        self.answers = iter(answers)
        self.log = log

    def __bool__(self):
        self.log.append("truth")
        answer = next(self.answers)
        if isinstance(answer, Exception):
            raise answer
        return answer

    def __lt__(self, other):
        self.log.append("compare")
        return self


def choose(value, tail):
    if value or tail():
        return "yes"
    return "no"


def choose_guard(value, tail):
    match value:
        case _ if value or tail():
            return "yes"
    return "no"


def choose_generator(value, tail):
    # The selected arm is itself a condition in runtime-loaded source.
    if (value or tail()) if value is not None else False:
        yield "yes"
    else:
        yield "no"


def current():
    log = []

    def tail():
        log.append("tail")
        raise ValueError("tail")

    # Ordinary functions use boxed native code. Generator bodies also exercise runtime source.
    for call in (choose, choose_guard, lambda value, tail: next(choose_generator(value, tail))):
        assert call(Probe([True, False], log), tail) == "yes"
        assert log == ["truth"]
        log.clear()
        for answer, message, events in (
            (False, "tail", ["truth", "tail"]),
            (ValueError("truth"), "truth", ["truth"]),
        ):
            try:
                call(Probe([answer], log), tail)
            except ValueError as error:
                assert error.args == (message,)
            else:
                raise AssertionError("expected ValueError")
            assert log == events
            log.clear()

    value = Probe([True, False], log)
    selected = value or tail()
    assert selected is value
    assert not selected
    assert log == ["truth", "truth"]
    log.clear()

    value = Probe([True], log)
    sentinel = object()
    assert ((value or tail()) and sentinel) is sentinel
    assert log == ["truth"]
    log.clear()

    value = Probe([False, True], log)
    if not (value < value < tail()):
        log.append("branch")
    assert log == ["compare", "truth", "branch"]
    log.clear()

    value = Probe([False, True], log)
    selected = (value < value < tail()) and sentinel
    assert selected is sentinel
    assert log == ["compare", "truth", "truth"]
