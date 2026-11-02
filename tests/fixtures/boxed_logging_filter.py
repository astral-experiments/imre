import io
import logging


def current():
    output = io.StringIO()
    handler = logging.StreamHandler(output)
    logger = logging.getLogger("app.worker")
    logger.propagate = False
    logger.setLevel(logging.INFO)
    logger.addHandler(handler)
    try:
        for name, message in (("app.w", "prefix"), ("app.worker.child", "child"),
                              ("app.worker", "exact"), ("app", "parent"), ("", "all")):
            filter_ = logging.Filter(name=name)
            logger.addFilter(filter_)
            logger.info(message)
            logger.removeFilter(filter_)
        assert output.getvalue() == "exact\nparent\nall\n"
    finally:
        logger.removeHandler(handler)
        handler.close()
