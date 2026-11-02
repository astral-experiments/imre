import io
import logging.config
import socketserver
from importlib import import_module


def current():
    assert import_module("logging.config") is logging.config
    assert logging.config.ThreadingTCPServer is socketserver.ThreadingTCPServer
    output = io.StringIO()
    handler = logging.StreamHandler(output)
    handler.name = "destination"
    logger = logging.getLogger("configured")
    logger.propagate = False
    logger.addHandler(handler)
    try:
        logging.config.dictConfig({
            "version": 1, "incremental": True,
            "handlers": {"destination": {"level": "WARNING"}},
            "loggers": {"configured": {"level": "INFO"}},
        })
        logger.info("hidden")
        logger.warning("visible")
        assert output.getvalue() == "visible\n"
        try:
            logging.config.dictConfig({
                "version": 1, "incremental": True,
                "handlers": {"missing": {"level": "INFO"}},
            })
        except ValueError as error:
            assert str(error) == "No handler found with name 'missing'"
        else:
            raise AssertionError("accepted an unregistered handler")
    finally:
        logger.removeHandler(handler)
        handler.close()
