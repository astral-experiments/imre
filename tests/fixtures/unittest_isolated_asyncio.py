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
        await asyncio.sleep(0)
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
