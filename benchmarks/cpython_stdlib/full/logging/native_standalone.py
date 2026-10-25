"""Run the original handler-removal loop on a directly constructed Logger."""
from native_encoding import SOURCE, base_fixture
from suite_native import method_bytes, native_method, testcase_fixture


CASE = "test.test_logging.LoggerTest.test_has_handlers"
SETUP = """
    logger: logging.Logger
    root_stream: io.StringIO

    def __init__(self) -> None:
        super().__init__()
        self.root_stream = configure_root()
        self.logger = logging.Logger(name="blah")
        self.logger.addHandler(RecordingHandler())
"""


def handler_case(toolchain, work):
    handle = method_bytes(SOURCE, "RecordingHandler", "handle").decode()
    recording = """
class RecordingHandler(logging.NullHandler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

""" + handle

    def execute(case, method):
        if (case.logger.name != "blah" or case.logger.parent is not None
                or case.logger.level != 0 or case.logger.handlers != [case.recording]
                or type(case.recording).__name__ != "RecordingHandler" or case.recording.records):
            raise AssertionError("the original standalone logger fixture changed")
        fixture = base_fixture(case) + recording + "\n" + testcase_fixture() + SETUP
        run = native_method(toolchain, work, SOURCE, "LoggerTest", CASE, fixture,
                            operation=("imre", "Find and remove the standalone logger's live handlers"))
        result = run(case, method)
        result["observation"]["evidence"]["fixture"] = (
            "Original CPython LoggerTest/BaseTest setup, teardown and cleanup; the child "
            "recreates the real root stream, separate Logger('blah') and its RecordingHandler "
            "with the unchanged handle method")
        return result

    return execute
