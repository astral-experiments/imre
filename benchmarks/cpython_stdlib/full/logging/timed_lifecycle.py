"""Exercise descriptor ownership, backup selection and clock fields across a rotation."""
import datetime
import logging.handlers
import sys

path = sys.argv[1]
h = logging.handlers.TimedRotatingFileHandler(path, "s", backupCount=1, utc=True)
print(h.rolloverAt)
print(h.interval)
print(h.backupCount)
print(h.when)
print(h.suffix)
print(h.baseFilename)
print(h.shouldRollover(None))
old = h.stream
if old is None:
    raise AssertionError("handler did not open its file")
print(old.closed)
h.doRollover()
print(old.closed)
new = h.stream
if new is None:
    raise AssertionError("rotation did not reopen the file")
new.write("new line\n")
h.flush()
h.close()
print(new.closed)
print(h.stream is None)
h.close()

clock = datetime.time(1, 2, 3, 4, fold=1)
print(clock.fold)
print(clock.microsecond)
print(clock == datetime.time(1, 2, 3, 4))
print(clock < datetime.time(1, 2, 3, 5))
delayed = logging.handlers.TimedRotatingFileHandler(
    path, "midnight", interval=2, utc=True, delay=True, atTime=clock)
print(delayed.atTime is clock)
print(delayed.stream is None)
print(delayed.computeRollover(0))
print(delayed.computeRollover(-0.75))
delayed.close()
