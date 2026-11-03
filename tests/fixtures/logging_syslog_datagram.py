import logging
import socket
from logging.handlers import SysLogHandler

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as receiver:
    receiver.bind(("127.0.0.1", 0))
    receiver.settimeout(5)
    handler = SysLogHandler(receiver.getsockname())
    logger = logging.getLogger("slh")
    logger.addHandler(handler)
    try:
        for append_nul, ident, expected in (
            (True, "", b"<11>sp\xc3\xa4m\x00"),
            (False, "", b"<11>sp\xc3\xa4m"),
            (False, "häm-", b"<11>h\xc3\xa4m-sp\xc3\xa4m"),
        ):
            handler.append_nul = append_nul
            handler.ident = ident
            logger.error("späm")
            assert receiver.recv(128) == expected
    finally:
        logger.removeHandler(handler)
        handler.close()
