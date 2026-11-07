# CPython 3.14.5 Lib/unittest/mock.py
import threading as _threading

_timeout_unset = sentinel.TIMEOUT_UNSET

class _ThreadingMixin:

    DEFAULT_TIMEOUT = None

    def _get_child_mock(self, /, **kw):
        if isinstance(kw.get("parent"), _ThreadingMixin):
            kw["timeout"] = kw["parent"]._mock_wait_timeout
        elif isinstance(kw.get("_new_parent"), _ThreadingMixin):
            kw["timeout"] = kw["_new_parent"]._mock_wait_timeout
        return super()._get_child_mock(**kw)

    def __init__(self, *args, timeout=_timeout_unset, **kwargs):
        super().__init__(*args, **kwargs)
        if timeout is _timeout_unset:
            timeout = self.DEFAULT_TIMEOUT
        self.__dict__["_mock_event"] = _threading.Event()  # Event for any call
        self.__dict__["_mock_calls_events"] = []  # Events for each of the calls
        self.__dict__["_mock_calls_events_lock"] = _threading.Lock()
        self.__dict__["_mock_wait_timeout"] = timeout

    def _reset_mock_state(self, return_value, side_effect):
        # The bundled Mock resets each child in one iterative walk.
        super()._reset_mock_state(return_value, side_effect)
        self.__dict__["_mock_event"] = _threading.Event()
        self.__dict__["_mock_calls_events"] = []

    def __get_event(self, expected_args, expected_kwargs):
        with self._mock_calls_events_lock:
            for args, kwargs, event in self._mock_calls_events:
                if (args, kwargs) == (expected_args, expected_kwargs):
                    return event
            new_event = _threading.Event()
            self._mock_calls_events.append((expected_args, expected_kwargs, new_event))
        return new_event

    def _mock_call(self, *args, **kwargs):
        ret_value = super()._mock_call(*args, **kwargs)

        call_event = self.__get_event(args, kwargs)
        call_event.set()

        self._mock_event.set()

        return ret_value

    def wait_until_called(self, *, timeout=_timeout_unset):
        """Wait until the mock object is called.

        `timeout` - time to wait for in seconds, waits forever otherwise.
        Defaults to the constructor provided timeout.
        Use None to block undefinetively.
        """
        if timeout is _timeout_unset:
            timeout = self._mock_wait_timeout
        if not self._mock_event.wait(timeout=timeout):
            msg = (f"{self._mock_name or 'mock'} was not called before"
                   f" timeout({timeout}).")
            raise AssertionError(msg)

    def wait_until_any_call_with(self, *args, **kwargs):
        """Wait until the mock object is called with given args.

        Waits for the timeout in seconds provided in the constructor.
        """
        event = self.__get_event(args, kwargs)
        if not event.wait(timeout=self._mock_wait_timeout):
            expected_string = self._format_mock_call_signature(args, kwargs)
            raise AssertionError(f'{expected_string} call not found')


class ThreadingMock(_ThreadingMixin, Mock):
    """
    A mock that can be used to wait until on calls happening
    in a different thread.

    The constructor can take a `timeout` argument which
    controls the timeout in seconds for all `wait` calls of the mock.

    You can change the default timeout of all instances via the
    `ThreadingMock.DEFAULT_TIMEOUT` attribute.

    If no timeout is set, it will block undefinetively.
    """
    _mock_auto_magics = MagicMock._mock_auto_magics
