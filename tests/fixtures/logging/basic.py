import logging
import sys
from enum import StrEnum
from pathlib import Path


class Mode(StrEnum):
    CHECK = "check"

logger = logging.getLogger(__name__)
def value() -> int:
    print("evaluated")
    return 1 << 80

logger.info("disabled %d", value())
sys.stderr.write("prefix,")
logger.warning("fallback warning")
sys.stderr.write("suffix")
sys.stderr.flush()
logging.basicConfig(level=logging.DEBUG, format="%(asctime)s %(levelname)s %(message)s", datefmt="clock")
logger.debug("Found %s (%s)", "λ", "file")
logger.warning("progress %d/%d [%r] %%", 3, value(), "λ'\n")
logging.getLogger("httpx").setLevel(logging.WARNING)
print(logging.getLogger("httpx.child").getEffectiveLevel())
print(logging.getLogger("httpx").name)
print(logging.getLogger("httpx").isEnabledFor(logging.INFO))
logging.basicConfig(level=logging.ERROR)
logger.info("still here")

root = logging.getLogger()
root.setLevel(1 << 80)
print(logger.isEnabledFor(1 << 81))
print(logger.isEnabledFor(1 << 79))
logging.basicConfig(force=True, level=10, format="%(levelno)d:%(name)s:%(message)s")
logger.log(33, "%% unchanged without arguments")
parent = logging.getLogger("after.child")
print(parent.parent is root)
logging.getLogger("after").setLevel(50)
print(parent.parent is logging.getLogger("after"))
print(parent.getEffectiveLevel())
try:
    logging.basicConfig(force=True, format="no fields")
except ValueError as error:
    print(str(error))
logger.warning("fallback after invalid configuration")
logging.debug("discarded",)
logging.warning("configured from %s", "convenience")
logging.log(35, "custom level")
logging.basicConfig(force=True, level="INFO")
logger.info("running %s in %s", Mode.CHECK, Path("workspace"))
logger.setLevel("WARN")
logger.warning("named threshold")
logging.basicConfig(force=True, level=30, format="%%(message)s")
logger.warning("escaped literal formatter")
logging.basicConfig(force=True, format="%(asctime)s|%(message)s", datefmt="a%%\0\0b%%\0")
logger.warning("NUL timestamp")
logging.getLogger("a").setLevel(29)
leaf = logging.getLogger("a..leaf")
print(leaf.parent is root)
print(leaf.getEffectiveLevel())
logging.getLogger("a.")
print(leaf.parent is logging.getLogger("a."))
print(leaf.getEffectiveLevel())


cached = logging.getLogger("cache..middle.leaf")
logging.getLogger("cache").setLevel(1)
print(cached.isEnabledFor(logging.INFO))
logging.getLogger("cache.")
print(cached.getEffectiveLevel())
print(cached.isEnabledFor(logging.INFO))
print(cached.isEnabledFor(logging.INFO + 1))
cached.setLevel(logging.NOTSET)
print(cached.isEnabledFor(logging.INFO))
