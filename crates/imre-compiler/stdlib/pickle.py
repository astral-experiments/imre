# Adapted from CPython 3.14.5, Lib/pickle.py (5607950ef232dad16d75c0cf53101d9649d89115).
# See stdlib/docstrings/LICENSE. The native initializer supplies PickleError,
# HIGHEST_PROTOCOL and dumps. This module implements the reading side.

from copyreg import _inverted_registry, _extension_cache
import sys
from sys import maxsize
from struct import unpack
import io
import codecs
import _compat_pickle

_NoValue = object()

class UnpicklingError(PickleError):
    """This exception is raised when there is a problem unpickling an object,
    such as a security violation.

    Note that other exceptions may also be raised during unpickling, including
    (but not necessarily limited to) AttributeError, EOFError, ImportError,
    and IndexError.

    """
    __module__ = "_pickle"

# Pickle opcodes.  See pickletools.py for extensive docs.  The listing
# here is in kind-of alphabetical order of 1-character pickle code.
# pickletools groups them by purpose.

MARK           = b'('   # push special markobject on stack
STOP           = b'.'   # every pickle ends with STOP
POP            = b'0'   # discard topmost stack item
POP_MARK       = b'1'   # discard stack top through topmost markobject
DUP            = b'2'   # duplicate top stack item
FLOAT          = b'F'   # push float object; decimal string argument
INT            = b'I'   # push integer or bool; decimal string argument
BININT         = b'J'   # push four-byte signed int
BININT1        = b'K'   # push 1-byte unsigned int
LONG           = b'L'   # push long; decimal string argument
BININT2        = b'M'   # push 2-byte unsigned int
NONE           = b'N'   # push None
PERSID         = b'P'   # push persistent object; id is taken from string arg
BINPERSID      = b'Q'   #  "       "         "  ;  "  "   "     "  stack
REDUCE         = b'R'   # apply callable to argtuple, both on stack
STRING         = b'S'   # push string; NL-terminated string argument
BINSTRING      = b'T'   # push string; counted binary string argument
SHORT_BINSTRING= b'U'   #  "     "   ;    "      "       "      " < 256 bytes
UNICODE        = b'V'   # push Unicode string; raw-unicode-escaped'd argument
BINUNICODE     = b'X'   #   "     "       "  ; counted UTF-8 string argument
APPEND         = b'a'   # append stack top to list below it
BUILD          = b'b'   # call __setstate__ or __dict__.update()
GLOBAL         = b'c'   # push self.find_class(modname, name); 2 string args
DICT           = b'd'   # build a dict from stack items
EMPTY_DICT     = b'}'   # push empty dict
APPENDS        = b'e'   # extend list on stack by topmost stack slice
GET            = b'g'   # push item from memo on stack; index is string arg
BINGET         = b'h'   #   "    "    "    "   "   "  ;   "    " 1-byte arg
INST           = b'i'   # build & push class instance
LONG_BINGET    = b'j'   # push item from memo on stack; index is 4-byte arg
LIST           = b'l'   # build list from topmost stack items
EMPTY_LIST     = b']'   # push empty list
OBJ            = b'o'   # build & push class instance
PUT            = b'p'   # store stack top in memo; index is string arg
BINPUT         = b'q'   #   "     "    "   "   " ;   "    " 1-byte arg
LONG_BINPUT    = b'r'   #   "     "    "   "   " ;   "    " 4-byte arg
SETITEM        = b's'   # add key+value pair to dict
TUPLE          = b't'   # build tuple from topmost stack items
EMPTY_TUPLE    = b')'   # push empty tuple
SETITEMS       = b'u'   # modify dict by adding topmost key+value pairs
BINFLOAT       = b'G'   # push float; arg is 8-byte float encoding

TRUE           = b'I01\n'  # not an opcode; see INT docs in pickletools.py
FALSE          = b'I00\n'  # not an opcode; see INT docs in pickletools.py

# Protocol 2

PROTO          = b'\x80'  # identify pickle protocol
NEWOBJ         = b'\x81'  # build object by applying cls.__new__ to argtuple
EXT1           = b'\x82'  # push object from extension registry; 1-byte index
EXT2           = b'\x83'  # ditto, but 2-byte index
EXT4           = b'\x84'  # ditto, but 4-byte index
TUPLE1         = b'\x85'  # build 1-tuple from stack top
TUPLE2         = b'\x86'  # build 2-tuple from two topmost stack items
TUPLE3         = b'\x87'  # build 3-tuple from three topmost stack items
NEWTRUE        = b'\x88'  # push True
NEWFALSE       = b'\x89'  # push False
LONG1          = b'\x8a'  # push long from < 256 bytes
LONG4          = b'\x8b'  # push really big long

# Protocol 3 (Python 3.x)

BINBYTES       = b'B'   # push bytes; counted binary string argument
SHORT_BINBYTES = b'C'   #  "     "   ;    "      "       "      " < 256 bytes

# Protocol 4

SHORT_BINUNICODE = b'\x8c'  # push short string; UTF-8 length < 256 bytes
BINUNICODE8      = b'\x8d'  # push very long string
BINBYTES8        = b'\x8e'  # push very long bytes string
EMPTY_SET        = b'\x8f'  # push empty set on the stack
ADDITEMS         = b'\x90'  # modify set by adding topmost stack items
FROZENSET        = b'\x91'  # build frozenset from topmost stack items
NEWOBJ_EX        = b'\x92'  # like NEWOBJ but work with keyword only arguments
STACK_GLOBAL     = b'\x93'  # same as GLOBAL but using names on the stacks
MEMOIZE          = b'\x94'  # store top of the stack in memo
FRAME            = b'\x95'  # indicate the beginning of a new frame

# Protocol 5

BYTEARRAY8       = b'\x96'  # push bytearray
NEXT_BUFFER      = b'\x97'  # push next out-of-band buffer
READONLY_BUFFER  = b'\x98'  # make top of stack readonly

class _Unframer:
    def __init__(self, file_read, file_readline):
        self.file_read = file_read
        self.file_readline = file_readline
        self.current_frame = None

    def read(self, n):
        if self.current_frame:
            data = self.current_frame.read(n)
            if not data and n != 0:
                self.current_frame = None
                return self.file_read(n)
            if len(data) < n:
                raise UnpicklingError(
                    "pickle exhausted before end of frame")
            return data
        else:
            return self.file_read(n)

    def read_exact(self, n):
        data = self.read(n)
        if len(data) != n:
            raise UnpicklingError("pickle data was truncated")
        return data

    def readline(self):
        if self.current_frame:
            data = self.current_frame.readline()
            if data:
                if not data.endswith(b'\n'):
                    raise UnpicklingError("pickle exhausted before end of frame")
                return data
            self.current_frame = None
        data = self.file_readline()
        if not data.endswith(b'\n'):
            raise UnpicklingError("pickle data was truncated")
        return data

    def load_frame(self, frame_size):
        if self.current_frame and self.current_frame.read() != b'':
            raise UnpicklingError(
                "beginning of a new frame before end of current frame")
        frame = self.file_read(frame_size)
        if len(frame) != frame_size:
            raise UnpicklingError("pickle data was truncated")
        self.current_frame = io.BytesIO(frame)


def _getattribute(obj, dotted_path):
    for subpath in dotted_path:
        obj = getattr(obj, subpath)
    return obj

def _tuple_items(value):
    # The C reader accesses tuple storage without calling subclass iteration hooks.
    if type(value) is tuple:
        return value
    return tuple.__getitem__(value, slice(None))


def decode_long(data):
    r"""Decode a long from a two's complement little-endian binary string.

    >>> decode_long(b'')
    0
    >>> decode_long(b"\xff\x00")
    255
    >>> decode_long(b"\xff\x7f")
    32767
    >>> decode_long(b"\x00\xff")
    -256
    >>> decode_long(b"\x00\x80")
    -32768
    >>> decode_long(b"\x80")
    -128
    >>> decode_long(b"\x7f")
    127
    """
    return int.from_bytes(data, byteorder='little', signed=True)

class _Unpickler:

    def __init__(self, file, *, fix_imports=True,
                 encoding="ASCII", errors="strict", buffers=None):
        """This takes a binary file for reading a pickle data stream.

        The protocol version of the pickle is detected automatically, so
        no proto argument is needed.

        The argument *file* must have two methods, a read() method that
        takes an integer argument, and a readline() method that requires
        no arguments.  Both methods should return bytes.  Thus *file*
        can be a binary file object opened for reading, an io.BytesIO
        object, or any other custom object that meets this interface.

        The file-like object must have two methods, a read() method
        that takes an integer argument, and a readline() method that
        requires no arguments.  Both methods should return bytes.
        Thus file-like object can be a binary file object opened for
        reading, a BytesIO object, or any other custom object that
        meets this interface.

        If *buffers* is not None, it should be an iterable of buffer-enabled
        objects that is consumed each time the pickle stream references
        an out-of-band buffer view.  Such buffers have been given in order
        to the *buffer_callback* of a Pickler object.

        If *buffers* is None (the default), then the buffers are taken
        from the pickle stream, assuming they are serialized there.
        It is an error for *buffers* to be None if the pickle stream
        was produced with a non-None *buffer_callback*.

        Other optional arguments are *fix_imports*, *encoding* and
        *errors*, which are used to control compatibility support for
        pickle stream generated by Python 2.  If *fix_imports* is True,
        pickle will try to map the old Python 2 names to the new names
        used in Python 3.  The *encoding* and *errors* tell pickle how
        to decode 8-bit string instances pickled by Python 2; these
        default to 'ASCII' and 'strict', respectively. *encoding* can be
        'bytes' to read these 8-bit string instances as bytes objects.
        """
        self._buffers = iter(buffers) if buffers is not None else None
        self._file_readline = file.readline
        self._file_read = file.read
        self.memo = {}
        self.encoding = encoding
        self.errors = errors
        self.proto = 0
        self.fix_imports = fix_imports

    def load(self):
        """Read a pickled object representation from the open file.

        Return the reconstituted object hierarchy specified in the file.
        """
        # Check whether Unpickler was initialized correctly. This is
        # only needed to mimic the behavior of _pickle.Unpickler.dump().
        if not hasattr(self, "_file_read"):
            raise UnpicklingError("Unpickler.__init__() was not called by "
                                  "%s.__init__()" % (self.__class__.__name__,))
        self._unframer = _Unframer(self._file_read, self._file_readline)
        self.read = self._unframer.read_exact
        self.readline = self._unframer.readline
        self.metastack = []
        self.stack = []
        self.append = self.stack.append
        self.proto = 0
        read = self._unframer.read
        dispatch = self.dispatch
        while True:
            key = read(1)
            if not key:
                raise EOFError("Ran out of input")
            handler = dispatch.get(key[0])
            if handler is None:
                op = key[0]
                char = chr(op) if 32 <= op < 127 and op not in (39, 92) else f"\\x{op:02x}"
                raise UnpicklingError(f"invalid load key, '{char}'.")
            value = handler(self)
            if key == b'.':  # STOP
                return value

    def _require_stack(self, count):
        if len(self.stack) < count:
            if self.metastack:
                raise UnpicklingError("unexpected MARK found")
            raise UnpicklingError("unpickling stack underflow")

    # Return a list of items pushed in the stack after last MARK instruction.
    def pop_mark(self):
        if not self.metastack:
            raise UnpicklingError("could not find MARK")
        items = self.stack
        self.stack = self.metastack.pop()
        self.append = self.stack.append
        return items

    def persistent_load(self, pid):
        raise UnpicklingError("unsupported persistent id encountered")

    dispatch = {}

    def load_proto(self):
        proto = self.read(1)[0]
        if not 0 <= proto <= HIGHEST_PROTOCOL:
            raise ValueError("unsupported pickle protocol: %d" % proto)
        self.proto = proto
    dispatch[PROTO[0]] = load_proto

    def load_frame(self):
        frame_size, = unpack('<Q', self.read(8))
        if frame_size > sys.maxsize:
            raise ValueError("frame size > sys.maxsize: %d" % frame_size)
        self._unframer.load_frame(frame_size)
    dispatch[FRAME[0]] = load_frame

    def load_persid(self):
        try:
            pid = self.readline()[:-1].decode("ascii")
        except UnicodeDecodeError:
            raise UnpicklingError(
                "persistent IDs in protocol 0 must be ASCII strings")
        self.append(self.persistent_load(pid))
    dispatch[PERSID[0]] = load_persid

    def load_binpersid(self):
        self._require_stack(1)
        pid = self.stack.pop()
        self.append(self.persistent_load(pid))
    dispatch[BINPERSID[0]] = load_binpersid

    def load_none(self):
        self.append(None)
    dispatch[NONE[0]] = load_none

    def load_false(self):
        self.append(False)
    dispatch[NEWFALSE[0]] = load_false

    def load_true(self):
        self.append(True)
    dispatch[NEWTRUE[0]] = load_true

    def load_int(self):
        data = self.readline()
        if data == FALSE[1:]:
            val = False
        elif data == TRUE[1:]:
            val = True
        else:
            val = int(data)
        self.append(val)
    dispatch[INT[0]] = load_int

    def load_binint(self):
        self.append(unpack('<i', self.read(4))[0])
    dispatch[BININT[0]] = load_binint

    def load_binint1(self):
        self.append(self.read(1)[0])
    dispatch[BININT1[0]] = load_binint1

    def load_binint2(self):
        self.append(unpack('<H', self.read(2))[0])
    dispatch[BININT2[0]] = load_binint2

    def load_long(self):
        val = self.readline()[:-1]
        if val and val[-1] == b'L'[0]:
            val = val[:-1]
        self.append(int(val))
    dispatch[LONG[0]] = load_long

    def load_long1(self):
        n = self.read(1)[0]
        data = self.read(n)
        self.append(decode_long(data))
    dispatch[LONG1[0]] = load_long1

    def load_long4(self):
        n, = unpack('<i', self.read(4))
        if n < 0:
            # Corrupt or hostile pickle -- we never write one like this
            raise UnpicklingError("LONG pickle has negative byte count")
        data = self.read(n)
        self.append(decode_long(data))
    dispatch[LONG4[0]] = load_long4

    def load_float(self):
        self.append(float(self.readline()[:-1]))
    dispatch[FLOAT[0]] = load_float

    def load_binfloat(self):
        self.append(unpack('>d', self.read(8))[0])
    dispatch[BINFLOAT[0]] = load_binfloat

    def _decode_string(self, value):
        # Used to allow strings from Python 2 to be decoded either as
        # bytes or Unicode strings.  This should be used only with the
        # STRING, BINSTRING and SHORT_BINSTRING opcodes.
        if self.encoding == "bytes":
            return value
        else:
            return value.decode(self.encoding, self.errors)

    def load_string(self):
        data = self.readline()[:-1]
        # Strip outermost quotes
        if len(data) >= 2 and data[0] == data[-1] and data[0] in b'"\'':
            data = data[1:-1]
        else:
            raise UnpicklingError("the STRING opcode argument must be quoted")
        self.append(self._decode_string(codecs.escape_decode(data)[0]))
    dispatch[STRING[0]] = load_string

    def load_binstring(self):
        # Deprecated BINSTRING uses signed 32-bit length
        len, = unpack('<i', self.read(4))
        if len < 0:
            raise UnpicklingError("BINSTRING pickle has negative byte count")
        data = self.read(len)
        self.append(self._decode_string(data))
    dispatch[BINSTRING[0]] = load_binstring

    def load_binbytes(self):
        len, = unpack('<I', self.read(4))
        if len > maxsize:
            raise UnpicklingError("BINBYTES exceeds system's maximum size "
                                  "of %d bytes" % maxsize)
        self.append(self.read(len))
    dispatch[BINBYTES[0]] = load_binbytes

    def load_unicode(self):
        self.append(str(self.readline()[:-1], 'raw-unicode-escape'))
    dispatch[UNICODE[0]] = load_unicode

    def load_binunicode(self):
        len, = unpack('<I', self.read(4))
        if len > maxsize:
            raise UnpicklingError("BINUNICODE exceeds system's maximum size "
                                  "of %d bytes" % maxsize)
        self.append(str(self.read(len), 'utf-8', 'surrogatepass'))
    dispatch[BINUNICODE[0]] = load_binunicode

    def load_binunicode8(self):
        len, = unpack('<Q', self.read(8))
        if len > maxsize:
            raise UnpicklingError("BINUNICODE8 exceeds system's maximum size "
                                  "of %d bytes" % maxsize)
        self.append(str(self.read(len), 'utf-8', 'surrogatepass'))
    dispatch[BINUNICODE8[0]] = load_binunicode8

    def load_binbytes8(self):
        len, = unpack('<Q', self.read(8))
        if len > maxsize:
            raise UnpicklingError("BINBYTES8 exceeds system's maximum size "
                                  "of %d bytes" % maxsize)
        self.append(self.read(len))
    dispatch[BINBYTES8[0]] = load_binbytes8

    def load_bytearray8(self):
        len, = unpack('<Q', self.read(8))
        if len > maxsize:
            raise UnpicklingError("BYTEARRAY8 exceeds system's maximum size "
                                  "of %d bytes" % maxsize)
        self.append(bytearray(self.read(len)))
    dispatch[BYTEARRAY8[0]] = load_bytearray8

    def load_next_buffer(self):
        if self._buffers is None:
            raise UnpicklingError("pickle stream refers to out-of-band data "
                                  "but no *buffers* argument was given")
        try:
            buf = next(self._buffers)
        except StopIteration:
            raise UnpicklingError("not enough out-of-band buffers")
        self.append(buf)
    dispatch[NEXT_BUFFER[0]] = load_next_buffer

    def load_readonly_buffer(self):
        self._require_stack(1)
        buf = self.stack[-1]
        with memoryview(buf) as m:
            if not m.readonly:
                self.stack[-1] = m.toreadonly()
    dispatch[READONLY_BUFFER[0]] = load_readonly_buffer

    def load_short_binstring(self):
        len = self.read(1)[0]
        data = self.read(len)
        self.append(self._decode_string(data))
    dispatch[SHORT_BINSTRING[0]] = load_short_binstring

    def load_short_binbytes(self):
        len = self.read(1)[0]
        self.append(self.read(len))
    dispatch[SHORT_BINBYTES[0]] = load_short_binbytes

    def load_short_binunicode(self):
        len = self.read(1)[0]
        self.append(str(self.read(len), 'utf-8', 'surrogatepass'))
    dispatch[SHORT_BINUNICODE[0]] = load_short_binunicode

    def load_tuple(self):
        items = self.pop_mark()
        self.append(tuple(items))
    dispatch[TUPLE[0]] = load_tuple

    def load_empty_tuple(self):
        self.append(())
    dispatch[EMPTY_TUPLE[0]] = load_empty_tuple

    def load_tuple1(self):
        self._require_stack(1)
        self.stack[-1] = (self.stack[-1],)
    dispatch[TUPLE1[0]] = load_tuple1

    def load_tuple2(self):
        self._require_stack(2)
        self.stack[-2:] = [(self.stack[-2], self.stack[-1])]
    dispatch[TUPLE2[0]] = load_tuple2

    def load_tuple3(self):
        self._require_stack(3)
        self.stack[-3:] = [(self.stack[-3], self.stack[-2], self.stack[-1])]
    dispatch[TUPLE3[0]] = load_tuple3

    def load_empty_list(self):
        self.append([])
    dispatch[EMPTY_LIST[0]] = load_empty_list

    def load_empty_dictionary(self):
        self.append({})
    dispatch[EMPTY_DICT[0]] = load_empty_dictionary

    def load_empty_set(self):
        self.append(set())
    dispatch[EMPTY_SET[0]] = load_empty_set

    def load_frozenset(self):
        items = self.pop_mark()
        self.append(frozenset(items))
    dispatch[FROZENSET[0]] = load_frozenset

    def load_list(self):
        items = self.pop_mark()
        self.append(items)
    dispatch[LIST[0]] = load_list

    def load_dict(self):
        items = self.pop_mark()
        if len(items) % 2:
            raise UnpicklingError("odd number of items for DICT")
        d = {items[i]: items[i+1]
             for i in range(0, len(items), 2)}
        self.append(d)
    dispatch[DICT[0]] = load_dict

    # INST and OBJ differ only in how they get a class object.  It's not
    # only sensible to do the rest in a common routine, the two routines
    # previously diverged and grew different bugs.
    # klass is the class to instantiate, and k points to the topmost mark
    # object, following which are the arguments for klass.__init__.
    def _instantiate(self, klass, args):
        if (args or not isinstance(klass, type) or
            hasattr(klass, "__getinitargs__")):
            value = klass(*args)
        else:
            value = klass.__new__(klass)
        self.append(value)

    def load_inst(self):
        module = self.readline()[:-1].decode("ascii")
        name = self.readline()[:-1].decode("ascii")
        klass = self.find_class(module, name)
        self._instantiate(klass, self.pop_mark())
    dispatch[INST[0]] = load_inst

    def load_obj(self):
        # Stack is ... markobject classobject arg1 arg2 ...
        args = self.pop_mark()
        if not args:
            raise UnpicklingError("unpickling stack underflow")
        cls = args.pop(0)
        self._instantiate(cls, args)
    dispatch[OBJ[0]] = load_obj

    def _newobj(self, use_kwargs):
        self._require_stack(3 if use_kwargs else 2)
        kwargs = self.stack.pop() if use_kwargs else None
        args = self.stack.pop()
        cls = self.stack.pop()
        opcode = "NEWOBJ_EX" if use_kwargs else "NEWOBJ"
        if not issubclass(type(cls), type):
            raise UnpicklingError(f"{opcode} class argument must be a type, not {type(cls).__name__}")
        if not issubclass(type(args), tuple):
            raise UnpicklingError(f"{opcode} args argument must be a tuple, not {type(args).__name__}")
        if use_kwargs and not issubclass(type(kwargs), dict):
            raise UnpicklingError(f"{opcode} kwargs argument must be a dict, not {type(kwargs).__name__}")
        # tp_new consumes the underlying tuple and dictionary, including for subclasses.
        args = _tuple_items(args)
        # A direct type lookup bypasses metaclass hooks where the descriptor is available.
        new = getattr(type, "__getattribute__", getattr)(cls, "__new__")
        if use_kwargs:
            if type(kwargs) is not dict:
                kwargs = {key: value for key, value in dict.items(kwargs)}
            obj = new(cls, *args, **kwargs)
        else:
            obj = new(cls, *args)
        self.append(obj)

    def load_newobj(self):
        self._newobj(False)
    dispatch[NEWOBJ[0]] = load_newobj

    def load_newobj_ex(self):
        self._newobj(True)
    dispatch[NEWOBJ_EX[0]] = load_newobj_ex

    def load_global(self):
        module = self.readline()[:-1].decode("utf-8")
        name = self.readline()[:-1].decode("utf-8")
        if not module or not name:
            raise UnpicklingError("pickle data was truncated")
        self.append(self.find_class(module, name))
    dispatch[GLOBAL[0]] = load_global

    def load_stack_global(self):
        self._require_stack(2)
        name = self.stack.pop()
        module = self.stack.pop()
        if type(name) is not str or type(module) is not str:
            raise UnpicklingError("STACK_GLOBAL requires str")
        self.append(self.find_class(module, name))
    dispatch[STACK_GLOBAL[0]] = load_stack_global

    def load_ext1(self):
        code = self.read(1)[0]
        self.get_extension(code)
    dispatch[EXT1[0]] = load_ext1

    def load_ext2(self):
        code, = unpack('<H', self.read(2))
        self.get_extension(code)
    dispatch[EXT2[0]] = load_ext2

    def load_ext4(self):
        code, = unpack('<i', self.read(4))
        self.get_extension(code)
    dispatch[EXT4[0]] = load_ext4

    def get_extension(self, code):
        obj = _extension_cache.get(code, _NoValue)
        if obj is not _NoValue:
            self.append(obj)
            return
        key = _inverted_registry.get(code)
        if not key:
            if code <= 0: # note that 0 is forbidden
                # Corrupt or hostile pickle.
                raise UnpicklingError("EXT specifies code <= 0")
            raise ValueError("unregistered extension code %d" % code)
        obj = self.find_class(*key)
        _extension_cache[code] = obj
        self.append(obj)

    def find_class(self, module, name):
        # Subclasses may override this.
        sys.audit('pickle.find_class', module, name)
        if self.proto < 3 and self.fix_imports:
            if (module, name) in _compat_pickle.NAME_MAPPING:
                module, name = _compat_pickle.NAME_MAPPING[(module, name)]
            elif module in _compat_pickle.IMPORT_MAPPING:
                module = _compat_pickle.IMPORT_MAPPING[module]
        __import__(module, level=0)
        if self.proto >= 4 and '.' in name:
            dotted_path = name.split('.')
            try:
                return _getattribute(sys.modules[module], dotted_path)
            except AttributeError:
                raise AttributeError(
                    f"Can't resolve path {name!r} on module {module!r}")
        else:
            return getattr(sys.modules[module], name)

    def load_reduce(self):
        self._require_stack(2)
        stack = self.stack
        args = stack.pop()
        func = stack[-1]
        if not issubclass(type(args), tuple):
            raise TypeError("argument list must be a tuple")
        stack[-1] = func(*_tuple_items(args))
    dispatch[REDUCE[0]] = load_reduce

    def load_pop(self):
        if self.stack:
            del self.stack[-1]
        elif self.metastack:
            self.pop_mark()
        else:
            raise UnpicklingError("unpickling stack underflow")
    dispatch[POP[0]] = load_pop

    def load_pop_mark(self):
        self.pop_mark()
    dispatch[POP_MARK[0]] = load_pop_mark

    def load_dup(self):
        self._require_stack(1)
        self.append(self.stack[-1])
    dispatch[DUP[0]] = load_dup

    def load_get(self):
        i = int(self.readline()[:-1])
        try:
            self.append(self.memo[i])
        except KeyError:
            msg = f'Memo value not found at index {i}'
            raise UnpicklingError(msg) from None
    dispatch[GET[0]] = load_get

    def load_binget(self):
        i = self.read(1)[0]
        try:
            self.append(self.memo[i])
        except KeyError as exc:
            msg = f'Memo value not found at index {i}'
            raise UnpicklingError(msg) from None
    dispatch[BINGET[0]] = load_binget

    def load_long_binget(self):
        i, = unpack('<I', self.read(4))
        try:
            self.append(self.memo[i])
        except KeyError as exc:
            msg = f'Memo value not found at index {i}'
            raise UnpicklingError(msg) from None
    dispatch[LONG_BINGET[0]] = load_long_binget

    def load_put(self):
        i = int(self.readline()[:-1])
        if i < 0:
            raise ValueError("negative PUT argument")
        self._require_stack(1)
        self.memo[i] = self.stack[-1]
    dispatch[PUT[0]] = load_put

    def load_binput(self):
        i = self.read(1)[0]
        if i < 0:
            raise ValueError("negative BINPUT argument")
        self._require_stack(1)
        self.memo[i] = self.stack[-1]
    dispatch[BINPUT[0]] = load_binput

    def load_long_binput(self):
        i, = unpack('<I', self.read(4))
        if i > maxsize:
            raise ValueError("negative LONG_BINPUT argument")
        self._require_stack(1)
        self.memo[i] = self.stack[-1]
    dispatch[LONG_BINPUT[0]] = load_long_binput

    def load_memoize(self):
        self._require_stack(1)
        memo = self.memo
        memo[len(memo)] = self.stack[-1]
    dispatch[MEMOIZE[0]] = load_memoize

    def _append_items(self, items):
        self._require_stack(1)
        if not items:
            return
        obj = self.stack[-1]
        extend = getattr(obj, "extend", _NoValue)
        if extend is not _NoValue:
            extend(items)
        else:
            append = obj.append
            for item in items:
                append(item)

    def load_append(self):
        self._require_stack(2)
        self._append_items([self.stack.pop()])
    dispatch[APPEND[0]] = load_append

    def load_appends(self):
        self._append_items(self.pop_mark())
    dispatch[APPENDS[0]] = load_appends

    def load_setitem(self):
        self._require_stack(3)
        stack = self.stack
        value = stack.pop()
        key = stack.pop()
        dict = stack[-1]
        dict[key] = value
    dispatch[SETITEM[0]] = load_setitem

    def load_setitems(self):
        items = self.pop_mark()
        self._require_stack(1)
        dict = self.stack[-1]
        if len(items) % 2:
            raise UnpicklingError("odd number of items for SETITEMS")
        for i in range(0, len(items), 2):
            dict[items[i]] = items[i + 1]
    dispatch[SETITEMS[0]] = load_setitems

    def load_additems(self):
        items = self.pop_mark()
        self._require_stack(1)
        if not items:
            return
        set_obj = self.stack[-1]
        if type(set_obj) is set:
            set_obj.update(items)
        elif issubclass(type(set_obj), set):
            set.update(set_obj, items)
        else:
            add = set_obj.add
            for item in items:
                add(item)
    dispatch[ADDITEMS[0]] = load_additems

    def load_build(self):
        self._require_stack(2)
        stack = self.stack
        state = stack.pop()
        inst = stack[-1]
        setstate = getattr(inst, "__setstate__", _NoValue)
        if setstate is not _NoValue:
            setstate(state)
            return
        if issubclass(type(state), tuple):
            state = _tuple_items(state)
        paired = type(state) is tuple and len(state) == 2
        if paired:
            state, slotstate = state
        if state is not None:
            if not issubclass(type(state), dict):
                raise UnpicklingError("state is not a dictionary")
            inst_dict = inst.__dict__
            for k, v in dict.items(state):
                if type(k) is str:
                    k = sys.intern(k)
                inst_dict[k] = v
        if paired:
            if not issubclass(type(slotstate), dict):
                raise UnpicklingError("slot state is not a dictionary")
            for k, v in dict.items(slotstate):
                setattr(inst, k, v)
    dispatch[BUILD[0]] = load_build

    def load_mark(self):
        self.metastack.append(self.stack)
        self.stack = []
        self.append = self.stack.append
    dispatch[MARK[0]] = load_mark

    def load_stop(self):
        self._require_stack(1)
        return self.stack.pop()
    dispatch[STOP[0]] = load_stop

def loads(data, /, *, fix_imports=True, encoding="ASCII", errors="strict",
           buffers=None):
    """Read and return an object from a pickle bytes-like object."""
    if data is None:
        raise TypeError("a bytes-like object is required, not 'NoneType'")
    file = io.BytesIO(data)
    fix_imports = bool(fix_imports)
    for name, option in (("encoding", encoding), ("errors", errors)):
        if not isinstance(option, str):
            raise TypeError(f"loads() argument '{name}' must be str")
        if "\x00" in option:
            raise ValueError("embedded null character")
    return _Unpickler(file, fix_imports=fix_imports, buffers=buffers,
                      encoding=encoding, errors=errors).load()
