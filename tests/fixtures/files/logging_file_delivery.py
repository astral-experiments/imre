import codecs
import io
import logging
from pathlib import Path


def exercise() -> None:
    path = Path("routed.log")
    logger = logging.getLogger("files.child")
    logger.propagate = False
    logger.setLevel(logging.DEBUG)
    handler = logging.FileHandler(path, encoding="utf-8", delay=True)
    handler.name = "output"
    handler.setLevel(logging.ERROR)
    handler.setFormatter(logging.Formatter("[%(name)s] %(message)s"))
    logger.addHandler(handler)
    print(isinstance(handler, logging.StreamHandler))
    print(isinstance(handler, logging.Handler))
    print(logging.getHandlerByName("output") is handler)
    print(handler.lock is not None)
    print(handler.stream is None)
    logger.warning("below threshold")
    print(path.exists())
    handler.filters.append(logging.Filter("another.logger"))
    entry = logging.makeLogRecord({"name": "rejected.logger"})
    print(handler.handle(entry) is False)
    print(handler.handle(entry) is not entry)
    logger.error("rejected by filter")
    print(path.exists())
    handler.filters.clear()
    handler.terminator = "!\n"
    logger.error("π")
    print(path.read_bytes())
    print(entry is handler.handle(entry))
    file = handler.stream
    if file is not None:
        print(file.fileno() >= 0)
        memory = io.StringIO()
        print(handler.setStream(memory) is file)
        print(file.closed)
        logger.error("replacement")
        print(repr(memory.getvalue()))
        handler.close()
        print(memory.closed)
        print(file.closed)
        file.close()
        print(file.closed)
    print(handler.stream is None)
    print(logging.getHandlerByName("output") is None)
    logger.removeHandler(handler)
    print(len(logger.handlers))
    path.unlink()

    delayed = logging.FileHandler(path, mode="w", delay=True)
    delayed.name = "without-file"
    delayed.close()
    delayed.emit(logging.makeLogRecord({"msg": "never open"}))
    print(path.exists())
    print(logging.getHandlerByName("without-file") is None)

    binary = io.BytesIO()
    writer = codecs.getwriter("cp1251")(binary)
    delayed.setStream(writer)
    delayed.emit(logging.makeLogRecord({"msg": "до"}))
    print(binary.getvalue())
    delayed.close()
    print(binary.closed)


exercise()
