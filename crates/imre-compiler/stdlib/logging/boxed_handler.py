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

# CPython 3.14.5 Lib/logging/__init__.py. See licenses/CPython.txt.
class Filterer(object):
    """
    A base class for loggers and handlers which allows them to share
    common code.
    """
    def __init__(self):
        """
        Initialize the list of filters to be an empty list.
        """
        self.filters = []

    def addFilter(self, filter):
        """
        Add the specified filter to this handler.
        """
        if not (filter in self.filters):
            self.filters.append(filter)

    def removeFilter(self, filter):
        """
        Remove the specified filter from this handler.
        """
        if filter in self.filters:
            self.filters.remove(filter)

    def filter(self, record):
        """
        Determine if a record is loggable by consulting all the filters.

        The default is to allow the record to be logged; any filter can veto
        this by returning a false value.
        If a filter attached to a handler returns a log record instance,
        then that instance is used in place of the original log record in
        any further processing of the event by that handler.
        If a filter returns any other true value, the original log record
        is used in any further processing of the event by that handler.

        If none of the filters return false values, this method returns
        a log record.
        If any of the filters return a false value, this method returns
        a false value.

        .. versionchanged:: 3.2

           Allow filters to be just callables.

        .. versionchanged:: 3.12
           Allow filters to return a LogRecord instead of
           modifying it in place.
        """
        for f in self.filters:
            if hasattr(f, 'filter'):
                result = f.filter(record)
            else:
                result = f(record) # assume callable - will raise if not
            if not result:
                return False
            if isinstance(result, LogRecord):
                record = result
        return record


class Handler(Filterer):
    """
    Handler instances dispatch logging events to specific destinations.

    The base handler class. Acts as a placeholder which defines the Handler
    interface. Handlers can optionally use Formatter instances to format
    records as desired. By default, no formatter is specified; in this case,
    the 'raw' message as determined by record.message is logged.
    """
    def __init__(self, level=NOTSET):
        """
        Initializes the instance - basically setting the formatter to None
        and the filter list to empty.
        """
        Filterer.__init__(self)
        self._name = None
        self.level = _checkLevel(level)
        self.formatter = None
        self._closed = False
        # Add the handler to the global _handlerList (for cleanup on shutdown)
        _addHandlerRef(self)
        self.createLock()

    def acquire(self):
        if self.lock:
            self.lock.acquire()

    def release(self):
        if self.lock:
            self.lock.release()

    def handle(self, record):
        """
        Conditionally emit the specified logging record.

        Emission depends on filters which may have been added to the handler.
        Wrap the actual emission of the record with acquisition/release of
        the I/O thread lock.

        Returns an instance of the log record that was emitted
        if it passed all filters, otherwise a false value is returned.
        """
        rv = self.filter(record)
        if isinstance(rv, LogRecord):
            record = rv
        if rv:
            with self.lock:
                self.emit(record)
        return rv
