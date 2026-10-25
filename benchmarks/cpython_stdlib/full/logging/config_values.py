"""Exercise live logger configuration, repeated calls and INI path errors."""

import logging
import logging.config
import sys


app = logging.getLogger("service")
worker = logging.getLogger("service.worker")
other = logging.getLogger("other")
worker.setLevel(logging.ERROR)
logging.addLevelName(15, "TRACE")
logging.config.dictConfig({
    "version": 1,
    "root": {"level": "WARNING", "handlers": []},
    "loggers": {"service": {"level": "TRACE", "propagate": False}},
})
print(app is logging.getLogger("service"))
print(app.level)
print(app.propagate)
print(worker.getEffectiveLevel())
print(worker.level)
print(other.disabled)
print(other.isEnabledFor(logging.CRITICAL))
worker.warning("first fallback")
logging.config.dictConfig({
    "version": True,
    "incremental": True,
    "loggers": {"other": {"level": "INFO"}, "service": {"level": "ERROR", "propagate": True}},
    "root": {"level": "DEBUG"},
})
print(other.disabled)
print(other.isEnabledFor(logging.INFO))
print(worker.getEffectiveLevel())
print(logging.getLogger().level)
try:
    logging.config.dictConfig({})
except ValueError as e:
    print(str(e))
try:
    logging.config.dictConfig({"version": "2"})
except ValueError as e:
    print(str(e))
try:
    logging.config.dictConfig({"version": 1, "incremental": True, "handlers": {"missing": {}}})
except ValueError as e:
    print(str(e))
try:
    logging.config.fileConfig(sys.argv[1])
except FileNotFoundError as e:
    print(str(e))
try:
    logging.config.fileConfig(sys.argv[2])
except RuntimeError as e:
    print(str(e))
logging.config.fileConfig(sys.argv[3], disable_existing_loggers=False, encoding="utf-8")
print(logging.getLogger().level)
print(app.level)
print(other.disabled)
