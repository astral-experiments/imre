from unittest import TestCase


class Message:
    def __init__(self, events, label):
        self.events = events
        self.label = label

    def __str__(self):
        self.events.append(self.label)
        return self.label


class Broken:
    def __str__(self):
        return 3


def current():
    case = TestCase()
    events: list[str] = []
    standard = Message(events, "standard")
    detail = Message(events, "détail")
    assert case._formatMessage(None, standard) is standard
    assert case._formatMessage(detail, standard) == "standard : détail"
    assert events == ["standard", "détail"]
    with case.assertRaises(TypeError):
        case._formatMessage(Broken(), standard)
    assert events == ["standard", "détail", "standard"]

    case.longMessage = False
    with case.assertRaises(AssertionError) as caught:
        case.assertTrue(False, detail)
    assert caught.exception.args[0] is detail
    assert case._formatMessage([], standard) is standard
    assert events == ["standard", "détail", "standard"]
