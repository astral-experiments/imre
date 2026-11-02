# Copyright 2001-2022 by Vinay Sajip. All Rights Reserved.
#
# Permission to use, copy, modify, and distribute this software and its
# documentation for any purpose and without fee is hereby granted,
# provided that the above copyright notice appear in all copies and that
# both that copyright notice and this permission notice appear in
# supporting documentation, and that the name of Vinay Sajip
# not be used in advertising or publicity pertaining to distribution
# of the software without specific, written prior permission.
# VINAY SAJIP DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING
# ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL
# VINAY SAJIP BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR
# ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER
# IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT
# OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

# Selected from CPython 3.14.5 Lib/logging/__init__.py. See licenses/CPython.txt.
import collections.abc
import io
import os
import sys
import threading
import time
import traceback

#
#_startTime is used as the base when calculating the relative time of events
#
_startTime = time.time_ns()

#
#raiseExceptions is used to see if exceptions during handling should be
#propagated
#
raiseExceptions = True

#
# If you don't want threading information in the log, set this to False
#
logThreads = True

#
# If you don't want multiprocessing information in the log, set this to False
#
logMultiprocessing = True

#
# If you don't want process information in the log, set this to False
#
logProcesses = True

#
# If you don't want asyncio task information in the log, set this to False
#
logAsyncioTasks = True

CRITICAL = 50
FATAL = CRITICAL
ERROR = 40
WARNING = 30
WARN = WARNING
INFO = 20
DEBUG = 10
NOTSET = 0

_levelToName = {
    CRITICAL: 'CRITICAL',
    ERROR: 'ERROR',
    WARNING: 'WARNING',
    INFO: 'INFO',
    DEBUG: 'DEBUG',
    NOTSET: 'NOTSET',
}
_nameToLevel = {
    'CRITICAL': CRITICAL,
    'FATAL': FATAL,
    'ERROR': ERROR,
    'WARN': WARNING,
    'WARNING': WARNING,
    'INFO': INFO,
    'DEBUG': DEBUG,
    'NOTSET': NOTSET,
}

def getLevelNamesMapping():
    return _nameToLevel.copy()

def getLevelName(level):
    """
    Return the textual or numeric representation of logging level 'level'.

    If the level is one of the predefined levels (CRITICAL, ERROR, WARNING,
    INFO, DEBUG) then you get the corresponding string. If you have
    associated levels with names using addLevelName then the name you have
    associated with 'level' is returned.

    If a numeric value corresponding to one of the defined levels is passed
    in, the corresponding string representation is returned.

    If a string representation of the level is passed in, the corresponding
    numeric value is returned.

    If no matching numeric or string value is passed in, the string
    'Level %s' % level is returned.
    """
    # See Issues #22386, #27937 and #29220 for why it's this way
    result = _levelToName.get(level)
    if result is not None:
        return result
    result = _nameToLevel.get(level)
    if result is not None:
        return result
    return "Level %s" % level

def addLevelName(level, levelName):
    """
    Associate 'levelName' with 'level'.

    This is used when converting levels to text during message formatting.
    """
    with _lock:
        _levelToName[level] = levelName
        _nameToLevel[levelName] = level

if hasattr(sys, "_getframe"):
    currentframe = lambda: sys._getframe(1)
else: #pragma: no cover
    def currentframe():
        """Return the frame object for the caller's stack frame."""
        try:
            raise Exception
        except Exception as exc:
            return exc.__traceback__.tb_frame.f_back


_srcfile = os.path.normcase(addLevelName.__code__.co_filename)

# _srcfile is only used in conjunction with sys._getframe().
# Setting _srcfile to None will prevent findCaller() from being called. This
# way, you can avoid the overhead of fetching caller information.

# The following is based on warnings._is_internal_frame. It makes sure that
# frames of the import mechanism are skipped when logging at module level and
# using a stacklevel value greater than one.
def _is_internal_frame(frame):
    """Signal whether the frame is a CPython or logging module internal."""
    filename = os.path.normcase(frame.f_code.co_filename)
    return filename == _srcfile or (
        "importlib" in filename and "_bootstrap" in filename
    )


def _checkLevel(level):
    if isinstance(level, int):
        rv = level
    elif str(level) == level:
        if level not in _nameToLevel:
            raise ValueError("Unknown level: %r" % level)
        rv = _nameToLevel[level]
    else:
        raise TypeError("Level not an integer or a valid string: %r"
                        % (level,))
    return rv


class LogRecord(object):
    """
    A LogRecord instance represents an event being logged.

    LogRecord instances are created every time something is logged. They
    contain all the information pertinent to the event being logged. The
    main information passed in is in msg and args, which are combined
    using str(msg) % args to create the message field of the record. The
    record also includes information such as when the record was created,
    the source line where the logging call was made, and any exception
    information to be logged.
    """
    def __init__(self, name, level, pathname, lineno,
                 msg, args, exc_info, func=None, sinfo=None, **kwargs):
        """
        Initialize a logging record with interesting information.
        """
        ct = time.time_ns()
        self.name = name
        self.msg = msg
        #
        # The following statement allows passing of a dictionary as a sole
        # argument, so that you can do something like
        #  logging.debug("a %(a)d b %(b)s", {'a':1, 'b':2})
        # Suggested by Stefan Behnel.
        # Note that without the test for args[0], we get a problem because
        # during formatting, we test to see if the arg is present using
        # 'if self.args:'. If the event being logged is e.g. 'Value is %d'
        # and if the passed arg fails 'if self.args:' then no formatting
        # is done. For example, logger.warning('Value is %d', 0) would log
        # 'Value is %d' instead of 'Value is 0'.
        # For the use case of passing a dictionary, this should not be a
        # problem.
        # Issue #21172: a request was made to relax the isinstance check
        # to hasattr(args[0], '__getitem__'). However, the docs on string
        # formatting still seem to suggest a mapping object is required.
        # Thus, while not removing the isinstance check, it does now look
        # for collections.abc.Mapping rather than, as before, dict.
        if (args and len(args) == 1 and isinstance(args[0], collections.abc.Mapping)
            and args[0]):
            args = args[0]
        self.args = args
        self.levelname = getLevelName(level)
        self.levelno = level
        self.pathname = pathname
        try:
            self.filename = os.path.basename(pathname)
            self.module = os.path.splitext(self.filename)[0]
        except (TypeError, ValueError, AttributeError):
            self.filename = pathname
            self.module = "Unknown module"
        self.exc_info = exc_info
        self.exc_text = None      # used to cache the traceback text
        self.stack_info = sinfo
        self.lineno = lineno
        self.funcName = func
        self.created = ct / 1e9  # ns to float seconds
        # Get the number of whole milliseconds (0-999) in the fractional part of seconds.
        # Eg: 1_677_903_920_999_998_503 ns --> 999_998_503 ns--> 999 ms
        # Convert to float by adding 0.0 for historical reasons. See gh-89047
        self.msecs = (ct % 1_000_000_000) // 1_000_000 + 0.0
        if self.msecs == 999.0 and int(self.created) != ct // 1_000_000_000:
            # ns -> sec conversion can round up, e.g:
            # 1_677_903_920_999_999_900 ns --> 1_677_903_921.0 sec
            self.msecs = 0.0

        self.relativeCreated = (ct - _startTime) / 1e6
        if logThreads:
            self.thread = threading.get_ident()
            self.threadName = threading.current_thread().name
        else: # pragma: no cover
            self.thread = None
            self.threadName = None
        if not logMultiprocessing: # pragma: no cover
            self.processName = None
        else:
            self.processName = 'MainProcess'
            mp = sys.modules.get('multiprocessing')
            if mp is not None:
                # Errors may occur if multiprocessing has not finished loading
                # yet - e.g. if a custom import hook causes third-party code
                # to run when multiprocessing calls import. See issue 8200
                # for an example
                try:
                    self.processName = mp.current_process().name
                except Exception: #pragma: no cover
                    pass
        if logProcesses and hasattr(os, 'getpid'):
            self.process = os.getpid()
        else:
            self.process = None

        self.taskName = None
        if logAsyncioTasks:
            asyncio = sys.modules.get('asyncio')
            if asyncio:
                try:
                    self.taskName = asyncio.current_task().get_name()
                except Exception:
                    pass

    def __repr__(self):
        return '<LogRecord: %s, %s, %s, %s, "%s">'%(self.name, self.levelno,
            self.pathname, self.lineno, self.msg)

    def getMessage(self):
        """
        Return the message for this LogRecord.

        Return the message for this LogRecord after merging any user-supplied
        arguments with the message.
        """
        msg = str(self.msg)
        if self.args:
            msg = msg % self.args
        return msg

#
#   Determine which class to use when instantiating log records.
#
_logRecordFactory = LogRecord

def setLogRecordFactory(factory):
    """
    Set the factory to be used when instantiating a log record.

    :param factory: A callable which will be called to instantiate
    a log record.
    """
    global _logRecordFactory
    _logRecordFactory = factory

def getLogRecordFactory():
    """
    Return the factory to be used when instantiating a log record.
    """

    return _logRecordFactory

def makeLogRecord(dict):
    """
    Make a LogRecord whose attributes are defined by the specified dictionary,
    This function is useful for converting a logging event received over
    a socket connection (which is sent as a dictionary) into a LogRecord
    instance.
    """
    rv = _logRecordFactory(None, None, "", 0, "", (), None, None)
    rv.__dict__.update(dict)
    return rv
