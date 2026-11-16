import contextvars
import sys
import unittest

assert "unittest.async_case" not in sys.modules
assert "IsolatedAsyncioTestCase" not in vars(unittest)
assert "IsolatedAsyncioTestCase" in dir(unittest)

from unittest import IsolatedAsyncioTestCase
from unittest.async_case import IsolatedAsyncioTestCase as Direct

assert IsolatedAsyncioTestCase is Direct is vars(unittest)["IsolatedAsyncioTestCase"]
state = contextvars.ContextVar("state", default=None)
events = []


class Case(IsolatedAsyncioTestCase):
    def loop_factory(self):
        import asyncio
        loop = asyncio.new_event_loop()
        loop.slow_callback_duration = float("inf")
        return loop

    def run(self, result=None):
        events.append("run")
        return super().run(result)

    def setUp(self):
        state.set("setup")
        self.addAsyncCleanup(self.cleanup)

    async def asyncSetUp(self):
        self.assertEqual(state.get(), "setup")
        state.set("async setup")

    async def test_body(self):
        import asyncio
        self.assertEqual(state.get(), "async setup")
        self.loop = asyncio.get_running_loop()
        task = asyncio.current_task()
        coro = task.get_coro()
        self.assertEqual(coro.__name__, "test_body")
        self.assertEqual(coro.__qualname__, "Case.test_body")
        for field in ("__name__", "__qualname__"):
            previous = getattr(coro, field)
            name = "read" + chr(0xd800)
            setattr(coro, field, name)
            self.assertIs(getattr(coro, field), name)
            setattr(coro, field, previous)
        coro.__qualname__ = "connection.read"
        self.assertIn("coro=<connection.read()", repr(task))
        self.assertEqual(self.test_body.__qualname__, "Case.test_body")
        await asyncio.sleep(0)

        from unittest.mock import AsyncMock, call

        order = []

        async def double(value):
            order.append(value)
            return 2 * value

        mock = AsyncMock(side_effect=double)
        first = mock(1)
        second = mock(2)
        self.assertEqual(mock.call_args_list, [call(1), call(2)])
        mock.assert_not_awaited()
        self.assertEqual(order, [])
        second_result = await second
        first_result = await first
        self.assertEqual((second_result, first_result), (4, 2))
        self.assertEqual(order, [2, 1])
        self.assertEqual(mock.await_args_list, [call(2), call(1)])

        wrapped = AsyncMock(wraps=double)
        self.assertEqual(await wrapped(3), 6)
        wrapped.assert_awaited_once_with(3)

        class Transport(AsyncMock):
            pass

        transport = Transport()
        reply = await transport()
        for child in (reply, transport.send):
            self.assertIsInstance(child, AsyncMock)
            self.assertNotIsInstance(child, Transport)

        state.set("test")

    async def asyncTearDown(self):
        self.assertEqual(state.get(), "test")
        state.set("async teardown")

    def tearDown(self):
        self.assertEqual(state.get(), "async teardown")
        state.set("teardown")

    async def cleanup(self):
        self.assertEqual(state.get(), "teardown")
        events.append("cleanup")


case = Case("test_body")
result = unittest.TestResult()
unittest.TestSuite([case]).run(result)
assert result.testsRun == 1 and result.wasSuccessful(), (result.errors, result.failures)
assert events == ["run", "cleanup"]
assert case.loop.is_closed()
assert state.get() is None
