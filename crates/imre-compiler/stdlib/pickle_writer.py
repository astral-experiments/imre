# Adapted from CPython 3.14.5, Lib/pickle.py (5607950ef232dad16d75c0cf53101d9649d89115).
# See stdlib/docstrings/LICENSE. This source shares the boxed pickle module's
# opcode constants, native exceptions, and reader helpers.

from types import FunctionType
from copyreg import dispatch_table, _extension_registry
from itertools import batched
from functools import partial
from struct import pack

try:
    from _pickle import PickleBuffer
except ImportError:
    _HAVE_PICKLE_BUFFER = False
else:
    _HAVE_PICKLE_BUFFER = True

_tuplesize2code = [EMPTY_TUPLE, TUPLE1, TUPLE2, TUPLE3]


class _Framer:

    _FRAME_SIZE_MIN = 4
    _FRAME_SIZE_TARGET = 64 * 1024

    def __init__(self, file_write):
        self.file_write = file_write
        self.current_frame = None
        self._exporting = None

    def start_framing(self):
        self.current_frame = bytearray()

    def end_framing(self):
        if self.current_frame:
            self.commit_frame(force=True)
            self.current_frame = None

    def commit_frame(self, force=False):
        if self.current_frame is not None:
            f = self.current_frame
            if len(f) >= self._FRAME_SIZE_TARGET or force:
                data = memoryview(f)
                write = self.file_write
                previous = self._exporting
                self._exporting = f
                try:
                    if len(data) >= self._FRAME_SIZE_MIN:
                        # Issue a single call to the write method of the underlying
                        # file object for the frame opcode with the size of the
                        # frame. The concatenation is expected to be less expensive
                        # than issuing an additional call to write.
                        write(FRAME + pack("<Q", len(data)))

                    # Issue a separate call to write to append the frame
                    # contents without concatenation to the above to avoid a
                    # memory copy.
                    write(data)
                finally:
                    self._exporting = previous

                # Start the new frame with a new buffer so that
                # the file object can have delayed access to the previous frame
                # contents via an unreleased memoryview of the previous buffer.
                self.current_frame = bytearray()

    def write(self, data):
        frame = self.current_frame
        if frame is None:
            return self.file_write(data)
        if self._exporting is frame:
            raise BufferError("Existing exports of data: object cannot be re-sized")
        if type(data) is not bytes:
            # Preserve BytesIO.write's buffer conversion for non-opcode inputs.
            stream = io.BytesIO()
            stream.write(data)
            data = stream.getvalue()
        frame.extend(data)
        return len(data)

    def write_large_bytes(self, header, payload):
        write = self.file_write
        if self.current_frame is not None:
            # Terminate the current frame and flush it to the file.
            self.commit_frame(force=True)

        # Perform direct write of the header and payload of the large binary
        # object. Be careful not to concatenate the header and the payload
        # prior to calling 'write' as we do not want to allocate a large
        # temporary bytes object.
        # We intentionally do not insert a protocol 4 frame opcode to make
        # it possible to optimize file.read calls in the loader.
        write(header)
        write(payload)


def whichmodule(obj, name):
    """Find the module an object belong to."""
    dotted_path = name.split('.')
    module_name = getattr(obj, '__module__', None)
    if '<locals>' in dotted_path:
        raise PicklingError(f"Can't pickle local object {obj!r}")
    if module_name is None:
        # Protect the iteration by using a list copy of sys.modules against dynamic
        # modules that trigger imports of other modules upon calls to getattr.
        for module_name, module in sys.modules.copy().items():
            if (module_name == '__main__'
                or module_name == '__mp_main__'  # bpo-42406
                or module is None):
                continue
            try:
                if _getattribute(module, dotted_path) is obj:
                    return module_name
            except AttributeError:
                pass
        module_name = '__main__'

    try:
        __import__(module_name, level=0)
        module = sys.modules[module_name]
    except (ImportError, ValueError, KeyError) as exc:
        raise PicklingError(f"Can't pickle {obj!r}: {exc!s}")
    try:
        if _getattribute(module, dotted_path) is obj:
            return module_name
    except AttributeError:
        raise PicklingError(f"Can't pickle {obj!r}: "
                            f"it's not found as {module_name}.{name}")

    raise PicklingError(
        f"Can't pickle {obj!r}: it's not the same object as {module_name}.{name}")


def encode_long(x):
    r"""Encode a long to a two's complement little-endian binary string.
    Note that 0 is a special case, returning an empty string, to save a
    byte in the LONG1 pickling context.

    >>> encode_long(0)
    b''
    >>> encode_long(255)
    b'\xff\x00'
    >>> encode_long(32767)
    b'\xff\x7f'
    >>> encode_long(-256)
    b'\x00\xff'
    >>> encode_long(-32768)
    b'\x00\x80'
    >>> encode_long(-128)
    b'\x80'
    >>> encode_long(127)
    b'\x7f'
    >>>
    """
    if x == 0:
        return b''
    nbytes = (x.bit_length() >> 3) + 1
    result = x.to_bytes(nbytes, byteorder='little', signed=True)
    if x < 0 and nbytes > 1:
        if result[-1] == 0xff and (result[-2] & 0x80) != 0:
            result = result[:-1]
    return result


def _T(obj):
    cls = type(obj)
    module = cls.__module__
    if module in (None, 'builtins', '__main__'):
        return cls.__qualname__
    return f'{module}.{cls.__qualname__}'


class _Pickler:

    def __init__(self, file, protocol=None, *, fix_imports=True,
                 buffer_callback=None):
        """This takes a binary file for writing a pickle data stream.

        The optional *protocol* argument tells the pickler to use the
        given protocol; supported protocols are 0, 1, 2, 3, 4 and 5.
        The default protocol is 5. It was introduced in Python 3.8, and
        is incompatible with previous versions.

        Specifying a negative protocol version selects the highest
        protocol version supported.  The higher the protocol used, the
        more recent the version of Python needed to read the pickle
        produced.

        The *file* argument must have a write() method that accepts a
        single bytes argument. It can thus be a file object opened for
        binary writing, an io.BytesIO instance, or any other custom
        object that meets this interface.

        If *fix_imports* is True and *protocol* is less than 3, pickle
        will try to map the new Python 3 names to the old module names
        used in Python 2, so that the pickle data stream is readable
        with Python 2.

        If *buffer_callback* is None (the default), buffer views are
        serialized into *file* as part of the pickle stream.

        If *buffer_callback* is not None, then it can be called any number
        of times with a buffer view.  If the callback returns a false value
        (such as None), the given buffer is out-of-band; otherwise the
        buffer is serialized in-band, i.e. inside the pickle stream.

        It is an error if *buffer_callback* is not None and *protocol*
        is None or smaller than 5.
        """
        if protocol is None:
            protocol = DEFAULT_PROTOCOL
        if protocol < 0:
            protocol = HIGHEST_PROTOCOL
        elif not 0 <= protocol <= HIGHEST_PROTOCOL:
            raise ValueError("pickle protocol must be <= %d" % HIGHEST_PROTOCOL)
        if buffer_callback is not None and protocol < 5:
            raise ValueError("buffer_callback needs protocol >= 5")
        self._buffer_callback = buffer_callback
        try:
            self._file_write = file.write
        except AttributeError:
            raise TypeError("file must have a 'write' attribute")
        self.framer = _Framer(self._file_write)
        self.write = self.framer.write
        self._write_large_bytes = self.framer.write_large_bytes
        self.memo = {}
        self.proto = int(protocol)
        self.bin = protocol >= 1
        self.fast = 0
        self.fix_imports = fix_imports and protocol < 3

    def clear_memo(self):
        """Clears the pickler's "memo".

        The memo is the data structure that remembers which objects the
        pickler has already seen, so that shared or recursive objects
        are pickled by reference and not by value.  This method is
        useful when re-using picklers.
        """
        self.memo.clear()

    def dump(self, obj):
        """Write a pickled representation of obj to the open file."""
        # Check whether Pickler was initialized correctly. This is
        # only needed to mimic the behavior of _pickle.Pickler.dump().
        if not hasattr(self, "_file_write"):
            raise PicklingError("Pickler.__init__() was not called by "
                                "%s.__init__()" % (self.__class__.__name__,))
        if self.proto >= 2:
            self.write(PROTO + pack("<B", self.proto))
        if self.proto >= 4:
            self.framer.start_framing()
        self.save(obj)
        self.write(STOP)
        self.framer.end_framing()

    def memoize(self, obj):
        """Store an object in the memo."""

        # The Pickler memo is a dictionary mapping object ids to 2-tuples
        # that contain the Unpickler memo key and the object being memoized.
        # The memo key is written to the pickle and will become
        # the key in the Unpickler's memo.  The object is stored in the
        # Pickler memo so that transient objects are kept alive during
        # pickling.

        # The use of the Unpickler memo length as the memo key is just a
        # convention.  The only requirement is that the memo values be unique.
        # But there appears no advantage to any other scheme, and this
        # scheme allows the Unpickler memo to be implemented as a plain (but
        # growable) array, indexed by memo key.
        if self.fast:
            return
        assert id(obj) not in self.memo
        idx = len(self.memo)
        self.write(self.put(idx))
        self.memo[id(obj)] = idx, obj

    # Return a PUT (BINPUT, LONG_BINPUT) opcode string, with argument i.
    def put(self, idx):
        if self.proto >= 4:
            return MEMOIZE
        elif self.bin:
            if idx < 256:
                return BINPUT + pack("<B", idx)
            else:
                return LONG_BINPUT + pack("<I", idx)
        else:
            return PUT + repr(idx).encode("ascii") + b'\n'

    # Return a GET (BINGET, LONG_BINGET) opcode string, with argument i.
    def get(self, i):
        if self.bin:
            if i < 256:
                return BINGET + pack("<B", i)
            else:
                return LONG_BINGET + pack("<I", i)

        return GET + repr(i).encode("ascii") + b'\n'

    def save(self, obj, save_persistent_id=True):
        self.framer.commit_frame()

        # Check for persistent id (defined by a subclass)
        if save_persistent_id:
            pid = self.persistent_id(obj)
            if pid is not None:
                self.save_pers(pid)
                return

        # Check the memo
        x = self.memo.get(id(obj))
        if x is not None:
            self.write(self.get(x[0]))
            return

        rv = NotImplemented
        reduce = getattr(self, "reducer_override", _NoValue)
        if reduce is not _NoValue:
            rv = reduce(obj)

        if rv is NotImplemented:
            # Check the type dispatch table
            t = type(obj)
            f = self.dispatch.get(t)
            if f is not None:
                f(self, obj)  # Call unbound method with explicit self
                return

            # Check private dispatch table if any, or else
            # copyreg.dispatch_table
            reduce = getattr(self, 'dispatch_table', dispatch_table).get(t, _NoValue)
            if reduce is not _NoValue:
                rv = reduce(obj)
            else:
                # Check for a class with a custom metaclass; treat as regular
                # class
                if issubclass(t, type):
                    self.save_global(obj)
                    return

                # Check for a __reduce_ex__ method, fall back to __reduce__
                reduce = getattr(obj, "__reduce_ex__", _NoValue)
                if reduce is not _NoValue:
                    rv = reduce(self.proto)
                else:
                    reduce = getattr(obj, "__reduce__", _NoValue)
                    if reduce is not _NoValue:
                        rv = reduce()
                    else:
                        raise PicklingError(f"Can't pickle {_T(t)} object")

        # Check for string returned by reduce(), meaning "save as global"
        if isinstance(rv, str):
            self.save_global(obj, rv)
            return

        try:
            # Assert that reduce() returned a tuple
            if not isinstance(rv, tuple):
                raise PicklingError(f'__reduce__ must return a string or tuple, not {_T(rv)}')

            # Assert that it returned an appropriately sized tuple
            l = len(rv)
            if not (2 <= l <= 6):
                raise PicklingError("tuple returned by __reduce__ "
                                    "must contain 2 through 6 elements")

            # Save the reduce() output and finally memoize the object
            self.save_reduce(obj=obj, *rv)
        except BaseException as exc:
            exc.add_note(f'when serializing {_T(obj)} object')
            raise

    def persistent_id(self, obj):
        # This exists so a subclass can override it
        return None

    def save_pers(self, pid):
        # Save a persistent id reference
        if self.bin:
            self.save(pid, save_persistent_id=False)
            self.write(BINPERSID)
        else:
            try:
                self.write(PERSID + str(pid).encode("ascii") + b'\n')
            except UnicodeEncodeError:
                raise PicklingError(
                    "persistent IDs in protocol 0 must be ASCII strings")

    def save_reduce(self, func, args, state=None, listitems=None,
                    dictitems=None, state_setter=None, *, obj=None):
        # This API is called by some subclasses

        if not callable(func):
            raise PicklingError(f"first item of the tuple returned by __reduce__ "
                                f"must be callable, not {_T(func)}")
        if not isinstance(args, tuple):
            raise PicklingError(f"second item of the tuple returned by __reduce__ "
                                f"must be a tuple, not {_T(args)}")

        save = self.save
        write = self.write

        func_name = getattr(func, "__name__", "")
        if self.proto >= 2 and func_name == "__newobj_ex__":
            cls, args, kwargs = args
            if not hasattr(cls, "__new__"):
                raise PicklingError("first argument to __newobj_ex__() has no __new__")
            if obj is not None and cls is not obj.__class__:
                raise PicklingError(f"first argument to __newobj_ex__() "
                                    f"must be {obj.__class__!r}, not {cls!r}")
            if self.proto >= 4:
                try:
                    save(cls)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} class')
                    raise
                try:
                    save(args)
                    save(kwargs)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} __new__ arguments')
                    raise
                write(NEWOBJ_EX)
            else:
                func = partial(cls.__new__, cls, *args, **kwargs)
                try:
                    save(func)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} reconstructor')
                    raise
                save(())
                write(REDUCE)
        elif self.proto >= 2 and func_name == "__newobj__":
            # A __reduce__ implementation can direct protocol 2 or newer to
            # use the more efficient NEWOBJ opcode, while still
            # allowing protocol 0 and 1 to work normally.  For this to
            # work, the function returned by __reduce__ should be
            # called __newobj__, and its first argument should be a
            # class.  The implementation for __newobj__
            # should be as follows, although pickle has no way to
            # verify this:
            #
            # def __newobj__(cls, *args):
            #     return cls.__new__(cls, *args)
            #
            # Protocols 0 and 1 will pickle a reference to __newobj__,
            # while protocol 2 (and above) will pickle a reference to
            # cls, the remaining args tuple, and the NEWOBJ code,
            # which calls cls.__new__(cls, *args) at unpickling time
            # (see load_newobj below).  If __reduce__ returns a
            # three-tuple, the state from the third tuple item will be
            # pickled regardless of the protocol, calling __setstate__
            # at unpickling time (see load_build below).
            #
            # Note that no standard __newobj__ implementation exists;
            # you have to provide your own.  This is to enforce
            # compatibility with Python 2.2 (pickles written using
            # protocol 0 or 1 in Python 2.3 should be unpicklable by
            # Python 2.2).
            cls = args[0]
            if not hasattr(cls, "__new__"):
                raise PicklingError("first argument to __newobj__() has no __new__")
            if obj is not None and cls is not obj.__class__:
                raise PicklingError(f"first argument to __newobj__() "
                                    f"must be {obj.__class__!r}, not {cls!r}")
            args = args[1:]
            try:
                save(cls)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} class')
                raise
            try:
                save(args)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} __new__ arguments')
                raise
            write(NEWOBJ)
        else:
            try:
                save(func)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} reconstructor')
                raise
            try:
                save(args)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} reconstructor arguments')
                raise
            write(REDUCE)

        if obj is not None:
            # If the object is already in the memo, this means it is
            # recursive. In this case, throw away everything we put on the
            # stack, and fetch the object back from the memo.
            if id(obj) in self.memo:
                write(POP + self.get(self.memo[id(obj)][0]))
            else:
                self.memoize(obj)

        # More new special cases (that work with older protocols as
        # well): when __reduce__ returns a tuple with 4 or 5 items,
        # the 4th and 5th item should be iterators that provide list
        # items and dict items (as (key, value) tuples), or None.

        if listitems is not None:
            self._batch_appends(listitems, obj)

        if dictitems is not None:
            self._batch_setitems(dictitems, obj)

        if state is not None:
            if state_setter is None:
                try:
                    save(state)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} state')
                    raise
                write(BUILD)
            else:
                # If a state_setter is specified, call it instead of load_build
                # to update obj's with its previous state.
                # First, push state_setter and its tuple of expected arguments
                # (obj, state) onto the stack.
                try:
                    save(state_setter)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} state setter')
                    raise
                save(obj)  # simple BINGET opcode as obj is already memoized.
                try:
                    save(state)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} state')
                    raise
                write(TUPLE2)
                # Trigger a state_setter(obj, state) function call.
                write(REDUCE)
                # The purpose of state_setter is to carry-out an
                # inplace modification of obj. We do not care about what the
                # method might return, so its output is eventually removed from
                # the stack.
                write(POP)

    # Methods below this point are dispatched through the dispatch table

    dispatch = {}

    def save_none(self, obj):
        self.write(NONE)
    dispatch[type(None)] = save_none

    def save_bool(self, obj):
        if self.proto >= 2:
            self.write(NEWTRUE if obj else NEWFALSE)
        else:
            self.write(TRUE if obj else FALSE)
    dispatch[bool] = save_bool

    def save_long(self, obj):
        if self.bin:
            # If the int is small enough to fit in a signed 4-byte 2's-comp
            # format, we can store it more efficiently than the general
            # case.
            # First one- and two-byte unsigned ints:
            if obj >= 0:
                if obj <= 0xff:
                    self.write(BININT1 + pack("<B", obj))
                    return
                if obj <= 0xffff:
                    self.write(BININT2 + pack("<H", obj))
                    return
            # Next check for 4-byte signed ints:
            if -0x80000000 <= obj <= 0x7fffffff:
                self.write(BININT + pack("<i", obj))
                return
        if self.proto >= 2:
            encoded = encode_long(obj)
            n = len(encoded)
            if n < 256:
                self.write(LONG1 + pack("<B", n) + encoded)
            else:
                self.write(LONG4 + pack("<i", n) + encoded)
            return
        if -0x80000000 <= obj <= 0x7fffffff:
            self.write(INT + repr(obj).encode("ascii") + b'\n')
        else:
            self.write(LONG + repr(obj).encode("ascii") + b'L\n')
    dispatch[int] = save_long

    def save_float(self, obj):
        if self.bin:
            self.write(BINFLOAT + pack('>d', obj))
        else:
            self.write(FLOAT + repr(obj).encode("ascii") + b'\n')
    dispatch[float] = save_float

    def _save_bytes_no_memo(self, obj):
        # helper for writing bytes objects for protocol >= 3
        # without memoizing them
        assert self.proto >= 3
        n = len(obj)
        if n <= 0xff:
            self.write(SHORT_BINBYTES + pack("<B", n) + obj)
        elif n > 0xffffffff and self.proto >= 4:
            self._write_large_bytes(BINBYTES8 + pack("<Q", n), obj)
        elif n >= self.framer._FRAME_SIZE_TARGET:
            self._write_large_bytes(BINBYTES + pack("<I", n), obj)
        else:
            self.write(BINBYTES + pack("<I", n) + obj)

    def save_bytes(self, obj):
        if self.proto < 3:
            if not obj: # bytes object is empty
                self.save_reduce(bytes, (), obj=obj)
            else:
                self.save_reduce(codecs.encode,
                                 (str(obj, 'latin1'), 'latin1'), obj=obj)
            return
        self._save_bytes_no_memo(obj)
        self.memoize(obj)
    dispatch[bytes] = save_bytes

    def _save_bytearray_no_memo(self, obj):
        # helper for writing bytearray objects for protocol >= 5
        # without memoizing them
        assert self.proto >= 5
        n = len(obj)
        if n >= self.framer._FRAME_SIZE_TARGET:
            self._write_large_bytes(BYTEARRAY8 + pack("<Q", n), obj)
        else:
            self.write(BYTEARRAY8 + pack("<Q", n) + obj)

    def save_bytearray(self, obj):
        if self.proto < 5:
            if not obj:  # bytearray is empty
                self.save_reduce(bytearray, (), obj=obj)
            else:
                self.save_reduce(bytearray, (bytes(obj),), obj=obj)
            return
        self._save_bytearray_no_memo(obj)
        self.memoize(obj)
    dispatch[bytearray] = save_bytearray

    if _HAVE_PICKLE_BUFFER:
        def save_picklebuffer(self, obj):
            if self.proto < 5:
                raise PicklingError("PickleBuffer can only be pickled with "
                                    "protocol >= 5")
            with obj.raw() as m:
                if not m.contiguous:
                    raise PicklingError("PickleBuffer can not be pickled when "
                                        "pointing to a non-contiguous buffer")
                in_band = True
                if self._buffer_callback is not None:
                    in_band = bool(self._buffer_callback(obj))
                if in_band:
                    # Write data in-band
                    # XXX The C implementation avoids a copy here
                    buf = m.tobytes()
                    if m.readonly:
                        self._save_bytes_no_memo(buf)
                    else:
                        self._save_bytearray_no_memo(buf)
                    self.memoize(obj)
                else:
                    # Write data out-of-band
                    self.write(NEXT_BUFFER)
                    if m.readonly:
                        self.write(READONLY_BUFFER)

        dispatch[PickleBuffer] = save_picklebuffer

    def save_str(self, obj):
        if self.bin:
            encoded = obj.encode('utf-8', 'surrogatepass')
            n = len(encoded)
            if n <= 0xff and self.proto >= 4:
                self.write(SHORT_BINUNICODE + pack("<B", n) + encoded)
            elif n > 0xffffffff and self.proto >= 4:
                self._write_large_bytes(BINUNICODE8 + pack("<Q", n), encoded)
            elif n >= self.framer._FRAME_SIZE_TARGET:
                self._write_large_bytes(BINUNICODE + pack("<I", n), encoded)
            else:
                self.write(BINUNICODE + pack("<I", n) + encoded)
        else:
            # Escape what raw-unicode-escape doesn't, but memoize the original.
            tmp = obj.replace("\\", "\\u005c")
            tmp = tmp.replace("\0", "\\u0000")
            tmp = tmp.replace("\n", "\\u000a")
            tmp = tmp.replace("\r", "\\u000d")
            tmp = tmp.replace("\x1a", "\\u001a")  # EOF on DOS
            self.write(UNICODE + tmp.encode('raw-unicode-escape') + b'\n')
        self.memoize(obj)
    dispatch[str] = save_str

    def save_tuple(self, obj):
        if not obj: # tuple is empty
            if self.bin:
                self.write(EMPTY_TUPLE)
            else:
                self.write(MARK + TUPLE)
            return

        n = len(obj)
        save = self.save
        memo = self.memo
        if n <= 3 and self.proto >= 2:
            for i, element in enumerate(obj):
                try:
                    save(element)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} item {i}')
                    raise
            # Subtle.  Same as in the big comment below.
            if id(obj) in memo:
                get = self.get(memo[id(obj)][0])
                self.write(POP * n + get)
            else:
                self.write(_tuplesize2code[n])
                self.memoize(obj)
            return

        # proto 0 or proto 1 and tuple isn't empty, or proto > 1 and tuple
        # has more than 3 elements.
        write = self.write
        write(MARK)
        for i, element in enumerate(obj):
            try:
                save(element)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} item {i}')
                raise

        if id(obj) in memo:
            # Subtle.  d was not in memo when we entered save_tuple(), so
            # the process of saving the tuple's elements must have saved
            # the tuple itself:  the tuple is recursive.  The proper action
            # now is to throw away everything we put on the stack, and
            # simply GET the tuple (it's already constructed).  This check
            # could have been done in the "for element" loop instead, but
            # recursive tuples are a rare thing.
            get = self.get(memo[id(obj)][0])
            if self.bin:
                write(POP_MARK + get)
            else:   # proto 0 -- POP_MARK not available
                write(POP * (n+1) + get)
            return

        # No recursion.
        write(TUPLE)
        self.memoize(obj)

    dispatch[tuple] = save_tuple

    def save_list(self, obj):
        if self.bin:
            self.write(EMPTY_LIST)
        else:   # proto 0 -- can't use EMPTY_LIST
            self.write(MARK + LIST)

        self.memoize(obj)
        self._batch_appends(obj, obj)

    dispatch[list] = save_list

    _BATCHSIZE = 1000

    def _batch_appends(self, items, obj):
        # Helper to batch up APPENDS sequences
        save = self.save
        write = self.write

        if not self.bin:
            for i, x in enumerate(items):
                try:
                    save(x)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} item {i}')
                    raise
                write(APPEND)
            return

        start = 0
        for batch in batched(items, self._BATCHSIZE):
            batch_len = len(batch)
            if batch_len != 1:
                write(MARK)
                for i, x in enumerate(batch, start):
                    try:
                        save(x)
                    except BaseException as exc:
                        exc.add_note(f'when serializing {_T(obj)} item {i}')
                        raise
                write(APPENDS)
            else:
                try:
                    save(batch[0])
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} item {start}')
                    raise
                write(APPEND)
            start += batch_len

    def save_dict(self, obj):
        if self.bin:
            self.write(EMPTY_DICT)
        else:   # proto 0 -- can't use EMPTY_DICT
            self.write(MARK + DICT)

        self.memoize(obj)
        self._batch_setitems(obj.items(), obj)

    dispatch[dict] = save_dict

    def _batch_setitems(self, items, obj):
        # Helper to batch up SETITEMS sequences; proto >= 1 only
        save = self.save
        write = self.write

        if not self.bin:
            for k, v in items:
                save(k)
                try:
                    save(v)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} item {k!r}')
                    raise
                write(SETITEM)
            return

        for batch in batched(items, self._BATCHSIZE):
            if len(batch) != 1:
                write(MARK)
                for k, v in batch:
                    save(k)
                    try:
                        save(v)
                    except BaseException as exc:
                        exc.add_note(f'when serializing {_T(obj)} item {k!r}')
                        raise
                write(SETITEMS)
            else:
                k, v = batch[0]
                save(k)
                try:
                    save(v)
                except BaseException as exc:
                    exc.add_note(f'when serializing {_T(obj)} item {k!r}')
                    raise
                write(SETITEM)

    def save_set(self, obj):
        save = self.save
        write = self.write

        if self.proto < 4:
            self.save_reduce(set, (list(obj),), obj=obj)
            return

        write(EMPTY_SET)
        self.memoize(obj)

        for batch in batched(obj, self._BATCHSIZE):
            write(MARK)
            try:
                for item in batch:
                    save(item)
            except BaseException as exc:
                exc.add_note(f'when serializing {_T(obj)} element')
                raise
            write(ADDITEMS)
    dispatch[set] = save_set

    def save_frozenset(self, obj):
        save = self.save
        write = self.write

        if self.proto < 4:
            self.save_reduce(frozenset, (list(obj),), obj=obj)
            return

        write(MARK)
        try:
            for item in obj:
                save(item)
        except BaseException as exc:
            exc.add_note(f'when serializing {_T(obj)} element')
            raise

        if id(obj) in self.memo:
            # If the object is already in the memo, this means it is
            # recursive. In this case, throw away everything we put on the
            # stack, and fetch the object back from the memo.
            write(POP_MARK + self.get(self.memo[id(obj)][0]))
            return

        write(FROZENSET)
        self.memoize(obj)
    dispatch[frozenset] = save_frozenset

    def save_global(self, obj, name=None):
        write = self.write
        memo = self.memo

        if name is None:
            name = getattr(obj, '__qualname__', None)
            if name is None:
                name = obj.__name__

        module_name = whichmodule(obj, name)
        if self.proto >= 2:
            code = _extension_registry.get((module_name, name), _NoValue)
            if code is not _NoValue:
                if code <= 0xff:
                    data = pack("<B", code)
                    if data == b'\0':
                        # Should never happen in normal circumstances,
                        # since the type and the value of the code are
                        # checked in copyreg.add_extension().
                        raise RuntimeError("extension code 0 is out of range")
                    write(EXT1 + data)
                elif code <= 0xffff:
                    write(EXT2 + pack("<H", code))
                else:
                    write(EXT4 + pack("<i", code))
                return

        if self.proto >= 4:
            self.save(module_name)
            self.save(name)
            write(STACK_GLOBAL)
        elif '.' in name:
            # In protocol < 4, objects with multi-part __qualname__
            # are represented as
            # getattr(getattr(..., attrname1), attrname2).
            dotted_path = name.split('.')
            name = dotted_path.pop(0)
            save = self.save
            for attrname in dotted_path:
                save(getattr)
                if self.proto < 2:
                    write(MARK)
            self._save_toplevel_by_name(module_name, name)
            for attrname in dotted_path:
                save(attrname)
                if self.proto < 2:
                    write(TUPLE)
                else:
                    write(TUPLE2)
                write(REDUCE)
        else:
            self._save_toplevel_by_name(module_name, name)

        self.memoize(obj)

    def _save_toplevel_by_name(self, module_name, name):
        if self.proto >= 3:
            # Non-ASCII identifiers are supported only with protocols >= 3.
            encoding = "utf-8"
        else:
            if self.fix_imports:
                r_name_mapping = _compat_pickle.REVERSE_NAME_MAPPING
                r_import_mapping = _compat_pickle.REVERSE_IMPORT_MAPPING
                if (module_name, name) in r_name_mapping:
                    module_name, name = r_name_mapping[(module_name, name)]
                elif module_name in r_import_mapping:
                    module_name = r_import_mapping[module_name]
            encoding = "ascii"
        try:
            self.write(GLOBAL + bytes(module_name, encoding) + b'\n')
        except UnicodeEncodeError:
            raise PicklingError(
                f"can't pickle module identifier {module_name!r} using "
                f"pickle protocol {self.proto}")
        try:
            self.write(bytes(name, encoding) + b'\n')
        except UnicodeEncodeError:
            raise PicklingError(
                f"can't pickle global identifier {name!r} using "
                f"pickle protocol {self.proto}")

    def save_type(self, obj):
        if obj is type(None):
            return self.save_reduce(type, (None,), obj=obj)
        elif obj is type(NotImplemented):
            return self.save_reduce(type, (NotImplemented,), obj=obj)
        elif obj is type(...):
            return self.save_reduce(type, (...,), obj=obj)
        return self.save_global(obj)

    dispatch[FunctionType] = save_global
    dispatch[type] = save_type


Pickler = _Pickler
