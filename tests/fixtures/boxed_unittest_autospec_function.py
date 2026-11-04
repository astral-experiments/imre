from unittest import TestCase
from unittest.mock import DEFAULT, call, create_autospec


receipts = []


def deliver(*, order):
    receipts.append(order)
    return f"sent:{order}"


def checkout(send):
    return send(order="A17")


check = TestCase()
send = create_autospec(deliver, wraps=deliver)
assert type(send) is type(deliver)
with check.assertRaises(TypeError):
    send("A17")
with check.assertRaises(TypeError):
    send()
send.assert_not_called()

assert checkout(send) == "sent:A17"
send.return_value = None
send.side_effect = ["queued", DEFAULT]
assert send(order="B18") == "queued"
assert send(order="C19") is None
send.side_effect = None
send.return_value = DEFAULT
assert send(order="D20") == "sent:D20"
assert receipts == ["A17", "D20"]
assert send.mock_calls == [call(order=n) for n in ("A17", "B18", "C19", "D20")]
assert send.mock.call_args_list is send.call_args_list
send.mock.assert_called_with(order="D20")
