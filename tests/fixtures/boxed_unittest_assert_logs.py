import logging
import unittest


def current():
    case = unittest.TestCase()
    assert case.assertLogs.__qualname__ == "TestCase.assertLogs"
    assert case.assertNoLogs.__qualname__ == "TestCase.assertNoLogs"
    logger = logging.getLogger("nested.assertLogs")
    handlers, level, propagate = logger.handlers[:], logger.level, logger.propagate
    with case.assertLogs(logger, "INFO") as outer:
        outer_handlers = logger.handlers[:]
        with case.assertRaises(ZeroDivisionError):
            with case.assertNoLogs(logger.name, "ERROR") as value:
                assert value is None
                logger.warning("below the inner level")
                raise ZeroDivisionError
        assert logger.handlers == outer_handlers
        assert logger.level == logging.INFO
        assert not logger.propagate
        logger.info("back in the outer context")
    assert outer.output == ["INFO:nested.assertLogs:back in the outer context"]
    assert outer.records[0].name == logger.name
    assert (logger.handlers, logger.level, logger.propagate) == (handlers, level, propagate)
