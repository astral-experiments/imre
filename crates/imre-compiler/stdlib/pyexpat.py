"""Python interface to the native Expat event parser."""

import sys as _sys
from types import ModuleType as _ModuleType
import _imre_pyexpat as _native


class ExpatError(Exception):
    __module__ = "xml.parsers.expat"


error = ExpatError
version_info = _native.version_info()
EXPAT_VERSION = "expat_%d.%d.%d" % version_info
native_encoding = "UTF-8"

XML_PARAM_ENTITY_PARSING_NEVER = 0
XML_PARAM_ENTITY_PARSING_UNLESS_STANDALONE = 1
XML_PARAM_ENTITY_PARSING_ALWAYS = 2


def ErrorString(errno, /):
    return _native.error_string(errno)


def _error_module():
    module = _ModuleType("pyexpat.errors")
    module.codes = {}
    module.messages = {}
    # Expat's XML_Error enum starts at XML_ERROR_NONE = 0.
    names = (
        "NO_MEMORY SYNTAX NO_ELEMENTS INVALID_TOKEN UNCLOSED_TOKEN PARTIAL_CHAR "
        "TAG_MISMATCH DUPLICATE_ATTRIBUTE JUNK_AFTER_DOC_ELEMENT PARAM_ENTITY_REF "
        "UNDEFINED_ENTITY RECURSIVE_ENTITY_REF ASYNC_ENTITY BAD_CHAR_REF BINARY_ENTITY_REF "
        "ATTRIBUTE_EXTERNAL_ENTITY_REF MISPLACED_XML_PI UNKNOWN_ENCODING INCORRECT_ENCODING "
        "UNCLOSED_CDATA_SECTION EXTERNAL_ENTITY_HANDLING NOT_STANDALONE UNEXPECTED_STATE "
        "ENTITY_DECLARED_IN_PE FEATURE_REQUIRES_XML_DTD CANT_CHANGE_FEATURE_ONCE_PARSING "
        "UNBOUND_PREFIX UNDECLARING_PREFIX INCOMPLETE_PE XML_DECL TEXT_DECL PUBLICID "
        "SUSPENDED NOT_SUSPENDED ABORTED FINISHED SUSPEND_PE RESERVED_PREFIX_XML "
        "RESERVED_PREFIX_XMLNS RESERVED_NAMESPACE_URI INVALID_ARGUMENT NO_BUFFER "
        "AMPLIFICATION_LIMIT_BREACH NOT_STARTED"
    )
    # pyexpat retains newer error constants when the loaded Expat predates them.
    fallback = {
        41: "invalid argument",
        42: "a successful prior call to function XML_GetBuffer is required",
        43: "limit on input amplification factor (from DTD and entities) breached",
        44: "parser not started",
    }
    for code, name in enumerate(names.split(), 1):
        message = ErrorString(code) or fallback[code]
        setattr(module, "XML_ERROR_" + name, message)
        module.codes[message] = code
        module.messages[code] = message
    return module


errors = _error_module()
model = _ModuleType("pyexpat.model")
model.XML_CTYPE_EMPTY = 1
model.XML_CTYPE_ANY = 2
model.XML_CTYPE_MIXED = 3
model.XML_CTYPE_NAME = 4
model.XML_CTYPE_CHOICE = 5
model.XML_CTYPE_SEQ = 6
model.XML_CQUANT_NONE = 0
model.XML_CQUANT_OPT = 1
model.XML_CQUANT_REP = 2
model.XML_CQUANT_PLUS = 3
_sys.modules["pyexpat.errors"] = errors
_sys.modules["pyexpat.model"] = model

_handlers = (
    "StartElementHandler", "EndElementHandler",
    "StartNamespaceDeclHandler", "EndNamespaceDeclHandler",
    "ProcessingInstructionHandler", "CommentHandler", "DefaultHandlerExpand",
)
_default_intern = object()


class xmlparser:
    __slots__ = _handlers + (
        "_handle", "_intern", "_buffer_text", "_buffer_size",
        "_ordered_attributes", "_specified_attributes", "_namespace_prefixes",
        "_character_handler", "_ignore_unhandled_text", "_start_doctype_decl_handler",
    )

    def __new__(cls, *args, **kwargs):
        raise TypeError("cannot create 'pyexpat.xmlparser' instances")

    def _clear_handlers(self):
        for name in _handlers:
            setattr(self, name, None)
        self.CharacterDataHandler = None
        self._start_doctype_decl_handler = None

    @property
    def StartDoctypeDeclHandler(self):
        return self._start_doctype_decl_handler

    @StartDoctypeDeclHandler.setter
    def StartDoctypeDeclHandler(self, handler):
        self._start_doctype_decl_handler = handler

    @StartDoctypeDeclHandler.deleter
    def StartDoctypeDeclHandler(self):
        raise RuntimeError("Cannot delete attribute")

    @property
    def CharacterDataHandler(self):
        return self._character_handler

    @CharacterDataHandler.setter
    def CharacterDataHandler(self, value):
        _native.flush_buffer(self._handle, self)
        self._character_handler = value
        # Removing the text handler while a callback runs suppresses default
        # text delivery until the handler is assigned again outside a callback.
        self._ignore_unhandled_text = value is None and _native.in_callback(self._handle)

    def Parse(self, data, isfinal=False, /):
        result = _native.parse(self._handle, self, data, isfinal)
        if result is not None:
            code, message, line, column = result
            exc = ExpatError(f"{message}: line {line}, column {column}")
            exc.code = code
            exc.lineno = line
            exc.offset = column
            raise exc
        return 1

    def GetReparseDeferralEnabled(self):
        return _native.get_reparse_deferral(self._handle)

    def SetReparseDeferralEnabled(self, enabled, /):
        _native.set_reparse_deferral(self._handle, bool(enabled))

    @property
    def intern(self):
        return self._intern

    @property
    def namespace_prefixes(self):
        return self._namespace_prefixes

    @namespace_prefixes.setter
    def namespace_prefixes(self, value):
        enabled = bool(value)
        _native.set_namespace_prefixes(self._handle, enabled)
        # Expat ignores this setting after parsing starts, but the getter
        # still reports the last value assigned to the Python attribute.
        self._namespace_prefixes = enabled

    @namespace_prefixes.deleter
    def namespace_prefixes(self):
        raise RuntimeError("Cannot delete attribute")

    @property
    def ordered_attributes(self):
        return self._ordered_attributes

    @ordered_attributes.setter
    def ordered_attributes(self, value):
        self._ordered_attributes = bool(value)

    @property
    def specified_attributes(self):
        return self._specified_attributes

    @specified_attributes.setter
    def specified_attributes(self, value):
        self._specified_attributes = bool(value)

    @property
    def buffer_text(self):
        return self._buffer_text

    @buffer_text.setter
    def buffer_text(self, value):
        enabled = bool(value)
        if not enabled and self._buffer_text:
            _native.flush_buffer(self._handle, self)
        self._buffer_text = enabled

    @property
    def buffer_size(self):
        return self._buffer_size

    @buffer_size.setter
    def buffer_size(self, value):
        if not isinstance(value, int):
            raise TypeError("buffer_size must be an integer")
        if value <= 0:
            raise ValueError("buffer_size must be greater than zero")
        if value > 2147483647:
            raise ValueError("buffer_size must not be greater than 2147483647")
        if value != self._buffer_size:
            _native.flush_buffer(self._handle, self)
            self._buffer_size = value
            self._buffer_text = True

    @property
    def ErrorCode(self):
        return _native.error_code(self._handle)

    @property
    def CurrentLineNumber(self):
        return _native.position(self._handle)[0]

    @property
    def CurrentColumnNumber(self):
        return _native.position(self._handle)[1]

    @property
    def CurrentByteIndex(self):
        return _native.position(self._handle)[2]

    ErrorLineNumber = CurrentLineNumber
    ErrorColumnNumber = CurrentColumnNumber
    ErrorByteIndex = CurrentByteIndex


XMLParserType = xmlparser
del xmlparser


def ParserCreate(encoding=None, namespace_separator=None, intern=_default_intern):
    if encoding is not None and not isinstance(encoding, str):
        raise TypeError("ParserCreate() argument 'encoding' must be str or None, "
                        f"not {type(encoding).__name__}")
    if namespace_separator is not None and not isinstance(namespace_separator, str):
        raise TypeError("ParserCreate() argument 'namespace_separator' must be str or None, "
                        f"not {type(namespace_separator).__name__}")
    if intern is _default_intern:
        intern = {}
    elif intern is not None and not isinstance(intern, dict):
        raise TypeError("intern must be a dictionary")
    handle = _native.create(encoding, namespace_separator)
    parser = object.__new__(XMLParserType)
    parser._handle = handle
    parser._intern = intern
    parser._buffer_text = False
    parser._buffer_size = 8192
    parser._namespace_prefixes = False
    parser._ordered_attributes = False
    parser._specified_attributes = False
    parser._clear_handlers()
    return parser
