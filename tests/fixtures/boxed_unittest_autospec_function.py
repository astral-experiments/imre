import asyncio
import inspect
from unittest import TestCase
from unittest.mock import DEFAULT, Mock, call, create_autospec


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

orders = Mock(name="orders")
orders.send = send
assert orders.send is send
checkout(orders.send)
assert orders.mock_calls == [call.send(order="A17")]

audit = Mock(name="audit")
audit.attach_mock(send, "deliver")
assert audit.deliver is send
checkout(audit.deliver)
assert audit.method_calls == [call.deliver(order="A17")]
assert orders.mock_calls == [call.send(order="A17")]
assert "name='audit.deliver'" in repr(send.mock)


async def dispatch(*, order):
    return f"queued:{order}"


async def check_dispatch():
    submit = create_autospec(dispatch, side_effect=dispatch)
    assert inspect.iscoroutinefunction(submit)
    invalid = submit("A17")
    pending = submit(order="B18")
    with check.assertRaises(TypeError):
        await invalid
    submit.assert_not_called()
    submit.assert_not_awaited()
    reply = await pending
    assert reply == "queued:B18"
    submit.assert_awaited_once_with(order="B18")
    assert submit.mock.await_args_list is submit.await_args_list
    assert submit.mock_calls == [call(order="B18")]


asyncio.run(check_dispatch())
