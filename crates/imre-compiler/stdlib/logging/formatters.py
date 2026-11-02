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

# CPython 3.14.5 Lib/logging/__init__.py, formatter classes and default instance.
import io
import re
import time
import traceback
from string import Formatter as StrFormatter
from string import Template

#---------------------------------------------------------------------------
#   Formatter classes and functions
#---------------------------------------------------------------------------
_str_formatter = StrFormatter()
del StrFormatter


class PercentStyle(object):

    default_format = '%(message)s'
    asctime_format = '%(asctime)s'
    asctime_search = '%(asctime)'
    validation_pattern = re.compile(r'%\(\w+\)[#0+ -]*(\*|\d+)?(\.(\*|\d+))?[diouxefgcrsa%]', re.I)

    def __init__(self, fmt, *, defaults=None):
        self._fmt = fmt or self.default_format
        self._defaults = defaults

    def usesTime(self):
        return self._fmt.find(self.asctime_search) >= 0

    def validate(self):
        """Validate the input format, ensure it matches the correct style"""
        if not self.validation_pattern.search(self._fmt):
            raise ValueError("Invalid format '%s' for '%s' style" % (self._fmt, self.default_format[0]))

    def _format(self, record):
        if defaults := self._defaults:
            values = defaults | record.__dict__
        else:
            values = record.__dict__
        return self._fmt % values

    def format(self, record):
        try:
            return self._format(record)
        except KeyError as e:
            raise ValueError('Formatting field not found in record: %s' % e)


class StrFormatStyle(PercentStyle):
    default_format = '{message}'
    asctime_format = '{asctime}'
    asctime_search = '{asctime'

    fmt_spec = re.compile(r'^(.?[<>=^])?[+ -]?#?0?(\d+|{\w+})?[,_]?(\.(\d+|{\w+}))?[bcdefgnosx%]?$', re.I)
    field_spec = re.compile(r'^(\d+|\w+)(\.\w+|\[[^]]+\])*$')

    def _format(self, record):
        if defaults := self._defaults:
            values = defaults | record.__dict__
        else:
            values = record.__dict__
        return self._fmt.format(**values)

    def validate(self):
        """Validate the input format, ensure it is the correct string formatting style"""
        fields = set()
        try:
            for _, fieldname, spec, conversion in _str_formatter.parse(self._fmt):
                if fieldname:
                    if not self.field_spec.match(fieldname):
                        raise ValueError('invalid field name/expression: %r' % fieldname)
                    fields.add(fieldname)
                if conversion and conversion not in 'rsa':
                    raise ValueError('invalid conversion: %r' % conversion)
                if spec and not self.fmt_spec.match(spec):
                    raise ValueError('bad specifier: %r' % spec)
        except ValueError as e:
            raise ValueError('invalid format: %s' % e)
        if not fields:
            raise ValueError('invalid format: no fields')


class StringTemplateStyle(PercentStyle):
    default_format = '${message}'
    asctime_format = '${asctime}'
    asctime_search = '${asctime}'

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._tpl = Template(self._fmt)

    def usesTime(self):
        fmt = self._fmt
        return fmt.find('$asctime') >= 0 or fmt.find(self.asctime_search) >= 0

    def validate(self):
        pattern = Template.pattern
        fields = set()
        for m in pattern.finditer(self._fmt):
            d = m.groupdict()
            if d['named']:
                fields.add(d['named'])
            elif d['braced']:
                fields.add(d['braced'])
            elif m.group(0) == '$':
                raise ValueError('invalid format: bare \'$\' not allowed')
        if not fields:
            raise ValueError('invalid format: no fields')

    def _format(self, record):
        if defaults := self._defaults:
            values = defaults | record.__dict__
        else:
            values = record.__dict__
        return self._tpl.substitute(**values)


BASIC_FORMAT = "%(levelname)s:%(name)s:%(message)s"

_STYLES = {
    '%': (PercentStyle, BASIC_FORMAT),
    '{': (StrFormatStyle, '{levelname}:{name}:{message}'),
    '$': (StringTemplateStyle, '${levelname}:${name}:${message}'),
}

class Formatter(object):
    """
    Formatter instances are used to convert a LogRecord to text.

    Formatters need to know how a LogRecord is constructed. They are
    responsible for converting a LogRecord to (usually) a string which can
    be interpreted by either a human or an external system. The base Formatter
    allows a formatting string to be specified. If none is supplied, the
    style-dependent default value, "%(message)s", "{message}", or
    "${message}", is used.

    The Formatter can be initialized with a format string which makes use of
    knowledge of the LogRecord attributes - e.g. the default value mentioned
    above makes use of the fact that the user's message and arguments are pre-
    formatted into a LogRecord's message attribute. Currently, the useful
    attributes in a LogRecord are described by:

    %(name)s            Name of the logger (logging channel)
    %(levelno)s         Numeric logging level for the message (DEBUG, INFO,
                        WARNING, ERROR, CRITICAL)
    %(levelname)s       Text logging level for the message ("DEBUG", "INFO",
                        "WARNING", "ERROR", "CRITICAL")
    %(pathname)s        Full pathname of the source file where the logging
                        call was issued (if available)
    %(filename)s        Filename portion of pathname
    %(module)s          Module (name portion of filename)
    %(lineno)d          Source line number where the logging call was issued
                        (if available)
    %(funcName)s        Function name
    %(created)f         Time when the LogRecord was created (time.time_ns() / 1e9
                        return value)
    %(asctime)s         Textual time when the LogRecord was created
    %(msecs)d           Millisecond portion of the creation time
    %(relativeCreated)d Time in milliseconds when the LogRecord was created,
                        relative to the time the logging module was loaded
                        (typically at application startup time)
    %(thread)d          Thread ID (if available)
    %(threadName)s      Thread name (if available)
    %(taskName)s        Task name (if available)
    %(process)d         Process ID (if available)
    %(processName)s     Process name (if available)
    %(message)s         The result of record.getMessage(), computed just as
                        the record is emitted
    """

    converter = time.localtime

    def __init__(self, fmt=None, datefmt=None, style='%', validate=True, *,
                 defaults=None):
        """
        Initialize the formatter with specified format strings.

        Initialize the formatter either with the specified format string, or a
        default as described above. Allow for specialized date formatting with
        the optional datefmt argument. If datefmt is omitted, you get an
        ISO8601-like (or RFC 3339-like) format.

        Use a style parameter of '%', '{' or '$' to specify that you want to
        use one of %-formatting, :meth:`str.format` (``{}``) formatting or
        :class:`string.Template` formatting in your format string.

        .. versionchanged:: 3.2
           Added the ``style`` parameter.
        """
        if style not in _STYLES:
            raise ValueError('Style must be one of: %s' % ','.join(
                             _STYLES.keys()))
        self._style = _STYLES[style][0](fmt, defaults=defaults)
        if validate:
            self._style.validate()

        self._fmt = self._style._fmt
        self.datefmt = datefmt

    default_time_format = '%Y-%m-%d %H:%M:%S'
    default_msec_format = '%s,%03d'

    def formatTime(self, record, datefmt=None):
        """
        Return the creation time of the specified LogRecord as formatted text.

        This method should be called from format() by a formatter which
        wants to make use of a formatted time. This method can be overridden
        in formatters to provide for any specific requirement, but the
        basic behaviour is as follows: if datefmt (a string) is specified,
        it is used with time.strftime() to format the creation time of the
        record. Otherwise, an ISO8601-like (or RFC 3339-like) format is used.
        The resulting string is returned. This function uses a user-configurable
        function to convert the creation time to a tuple. By default,
        time.localtime() is used; to change this for a particular formatter
        instance, set the 'converter' attribute to a function with the same
        signature as time.localtime() or time.gmtime(). To change it for all
        formatters, for example if you want all logging times to be shown in GMT,
        set the 'converter' attribute in the Formatter class.
        """
        ct = self.converter(record.created)
        if datefmt:
            s = time.strftime(datefmt, ct)
        else:
            s = time.strftime(self.default_time_format, ct)
            if self.default_msec_format:
                s = self.default_msec_format % (s, record.msecs)
        return s

    def formatException(self, ei):
        """
        Format and return the specified exception information as a string.

        This default implementation just uses
        traceback.print_exception()
        """
        sio = io.StringIO()
        tb = ei[2]
        # See issues #9427, #1553375. Commented out for now.
        #if getattr(self, 'fullstack', False):
        #    traceback.print_stack(tb.tb_frame.f_back, file=sio)
        traceback.print_exception(ei[0], ei[1], tb, limit=None, file=sio)
        s = sio.getvalue()
        sio.close()
        if s[-1:] == "\n":
            s = s[:-1]
        return s

    def usesTime(self):
        """
        Check if the format uses the creation time of the record.
        """
        return self._style.usesTime()

    def formatMessage(self, record):
        return self._style.format(record)

    def formatStack(self, stack_info):
        """
        This method is provided as an extension point for specialized
        formatting of stack information.

        The input data is a string as returned from a call to
        :func:`traceback.print_stack`, but with the last trailing newline
        removed.

        The base implementation just returns the value passed in.
        """
        return stack_info

    def format(self, record):
        """
        Format the specified record as text.

        The record's attribute dictionary is used as the operand to a
        string formatting operation which yields the returned string.
        Before formatting the dictionary, a couple of preparatory steps
        are carried out. The message attribute of the record is computed
        using LogRecord.getMessage(). If the formatting string uses the
        time (as determined by a call to usesTime(), formatTime() is
        called to format the event time. If there is exception information,
        it is formatted using formatException() and appended to the message.
        """
        record.message = record.getMessage()
        if self.usesTime():
            record.asctime = self.formatTime(record, self.datefmt)
        s = self.formatMessage(record)
        if record.exc_info:
            # Cache the traceback text to avoid converting it multiple times
            # (it's constant anyway)
            if not record.exc_text:
                record.exc_text = self.formatException(record.exc_info)
        if record.exc_text:
            if s[-1:] != "\n":
                s = s + "\n"
            s = s + record.exc_text
        if record.stack_info:
            if s[-1:] != "\n":
                s = s + "\n"
            s = s + self.formatStack(record.stack_info)
        return s

#
#   The default formatter to use when no other is specified
#
_defaultFormatter = Formatter()
