import asyncio
import concurrent.futures
import sys
from asyncio import events, format_helpers


def current():
    assert sys.modules["asyncio.events"] is events
    assert asyncio.Runner is sys.modules["asyncio.runners"].Runner
    assert concurrent.futures.Future is sys.modules["concurrent.futures._base"].Future
    policy = events._get_event_loop_policy()
    assert events._get_event_loop_policy() is policy
    other = type(policy)()
    assert policy._local is not other._local
    assert policy._local._loop is None and other._local._loop is None
    try:
        asyncio.get_running_loop()
    except RuntimeError as error:
        assert str(error) == "no running event loop"
    else:
        raise AssertionError("a loop is running outside an event loop")

    text = format_helpers._format_args_and_kwargs([list(range(8))], {"key": 7}, debug=True)
    assert text == "([0, 1, 2, 3, 4, 5, ...], key=7)"
    if sys.platform != "win32":
        try:
            from asyncio import windows_utils
        except ImportError as error:
            assert str(error) == "win32 only"
        else:
            raise AssertionError("Windows pipe helpers imported on POSIX")

    future = concurrent.futures.Future()
    future.set_result(7)
    assert future.result() == 7
    try:
        future.set_result(8)
    except concurrent.futures.InvalidStateError:
        pass
    else:
        raise AssertionError("a completed future accepted a second result")
    print("asyncio policy, callback formatting and completed future")
