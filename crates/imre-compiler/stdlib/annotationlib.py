"""Annotation formats and forward reference values from CPython 3.14.5 (PSF-2.0)."""

import enum
import sys
import types

__all__ = ["Format", "ForwardRef", "get_annotations", "annotations_to_string", "type_repr"]


class Format(enum.IntEnum):
    VALUE = 1
    VALUE_WITH_FAKE_GLOBALS = 2
    FORWARDREF = 3
    STRING = 4


def type_repr(value):
    """Convert a Python value to a format suitable for use with the STRING format.

    This is intended as a helper for tools that support the STRING format but do
    not have access to the code that originally produced the annotations. It uses
    repr() for most objects.

    """
    if value is None:
        return "None"
    if value is type(None):
        return "NoneType"
    if isinstance(value, (type, types.FunctionType, types.BuiltinFunctionType)):
        return (value.__qualname__ if value.__module__ == "builtins" else
                f"{value.__module__}.{value.__qualname__}")
    if value is ...:
        return "..."
    from string.templatelib import Template
    if isinstance(value, Template):
        return _template_repr(value)
    return repr(value)


_TEMPLATE_QUOTES = ("'", '"', '"""', "'''")


def _template_constant(value):
    def simple(item):
        if isinstance(item, (float, complex)):
            return repr(item).replace("inf", "1e309").replace("nan", "(1e309-1e309)")
        return repr(item)

    if isinstance(value, tuple):
        if len(value) == 1:
            return "".join(("(", simple(value[0]), ",)"))
        return "(" + ", ".join(simple(item) for item in value) + ")"
    return "..." if value is ... else simple(value)


def _template_constructor(template):
    parts = []
    for part in template:
        if isinstance(part, str):
            parts.append(_template_constant(part))
        else:
            arguments = (part.value, part.expression, part.conversion, part.format_spec)
            arguments = ", ".join(_template_constant(arg) for arg in arguments)
            parts.append("Interpolation(" + arguments + ")")
    return "Template(" + ", ".join(parts) + ")"


def _template_interpolation(expression, conversion, format_spec):
    parts = ["{ " if expression.startswith("{") else "{", expression]
    if conversion != -1:
        parts.extend(("!", chr(conversion)))
    if format_spec is not None:
        spec = format_spec.replace("{", "{{").replace("}", "}}")
        spec = spec.replace("\\", "\\\\").replace("'", "\\'").replace('"', '\\"')
        parts.extend((":", spec.replace("\n", "\\n")))
    parts.append("}")
    return "".join(parts)


def _quoted_template(parts):
    # Match _ast_unparse.Unparser._ftstring_helper: earlier parts restrict the
    # delimiters available to the rest of this literal.
    quotes = list(_TEMPLATE_QUOTES)
    rendered = []
    for part, literal in parts:
        if literal:
            # Single-character repr preserves printable characters and escapes
            # backslashes and nonprintable code points, including surrogates.
            escaped = "".join(repr(char)[1:-1] for char in part)
            choices = [q for q in quotes if q not in escaped]
            if not choices:
                text = repr(part)
                choices = [next((q for q in quotes if text[0] in q), text[0])]
                if choices[0] not in quotes:
                    # No common delimiter works for all parts. Match CPython's
                    # triple-single-quoted fallback and force repr's single quotes.
                    return "t'''" + "".join(
                        repr('"' + s)[2:-1] if is_literal else s for s, is_literal in parts
                    ) + "'''"
                escaped = text[1:-1]
            elif escaped:
                choices.sort(key=lambda q: q[0] == escaped[-1])
                if choices[0][0] == escaped[-1]:
                    escaped = escaped[:-1] + "\\" + escaped[-1]
            part, quotes = escaped, choices
        else:
            if "\n" in part:
                quotes = [q for q in quotes if len(q) == 3]
                assert quotes
            quotes = [q for q in quotes if q not in part] or quotes
        rendered.append(part)
    quote = quotes[0]
    return "t" + quote + "".join(rendered) + quote


def _template_repr(template):
    from typing import _check_annotation_syntax

    if any(part.expression.strip() == "" for part in template.interpolations):
        return _template_constructor(template)
    try:
        for part in template.interpolations:
            _check_annotation_syntax(f"({part.expression})")
    except SyntaxError:
        return _template_constructor(template)
    # Construct all fields before writing any text. Conversion, format specs and
    # subclasses of str can run Python code in both phases.
    fields = []
    for part in template:
        if isinstance(part, str):
            fields.append(part)
        else:
            fields.append((
                part.expression,
                ord(part.conversion) if part.conversion else -1,
                part.format_spec if part.format_spec else None,
            ))

    parts = []
    for field in fields:
        if type(field) is tuple:
            parts.append((_template_interpolation(*field), False))
        else:
            text = field.replace("{", "{{").replace("}", "}}")
            parts.append(("".join((text,)), True))
    return _quoted_template(parts)


def annotations_to_string(annotations):
    """Convert an annotation dict containing values to approximately the STRING format.

    Always returns a fresh a dictionary.
    """
    return {name: value if isinstance(value, str) else type_repr(value)
            for name, value in annotations.items()}


def _make_forward_ref(arg, module=None, owner=None, *, is_class=False):
    from typing import _validate_forward_ref
    # typing._make_forward_ref eagerly checks the syntax, without resolving names.
    source = f"({arg},)[0]" if arg.lstrip().startswith("*") else arg
    try:
        _validate_forward_ref(source)
    except SyntaxError:
        raise SyntaxError(f"Forward reference must be an expression -- got {arg!r}")
    return ForwardRef(arg, module=module, owner=owner, is_class=is_class)


class ForwardRef:
    """An unevaluated annotation with its defining scope."""

    __slots__ = (
        "__arg__",
        "__ast_node__",
        "__code__",
        "__stringifier_dict__",
        "__globals__",
        "__extra_names__",
        "__resolved_str_cache__",
        "__cell__",
        "__forward_module__",
        "__owner__",
        "__forward_is_argument__",
        "__forward_is_class__",
        "__weakref__",
    )

    def __init__(self, arg, *, module=None, owner=None, is_argument=True, is_class=False):
        if not isinstance(arg, str):
            raise TypeError(f"Forward reference must be a string -- got {arg!r}")
        self.__arg__ = arg
        self.__ast_node__ = None
        self.__code__ = None
        self.__globals__ = None
        self.__extra_names__ = None
        self.__resolved_str_cache__ = None
        self.__cell__ = None
        self.__forward_module__ = module
        self.__owner__ = owner
        self.__forward_is_argument__ = is_argument
        self.__forward_is_class__ = is_class

    def __init_subclass__(cls, /, *args, **kwargs):
        raise TypeError("Cannot subclass ForwardRef")

    @property
    def __forward_arg__(self):
        return self.__arg__

    @property
    def __resolved_str__(self):
        if self.__resolved_str_cache__ is None:
            if self.__ast_node__ is None:
                rendered = self.__forward_arg__
            else:
                # Preserve literal tokens and render captured values only when
                # the caller asks to see them. Their repr can execute user code.
                rendered = "".join(
                    part if type(part) is str else type_repr(part[1])
                    for part in self.__ast_node__
                )
            self.__resolved_str_cache__ = rendered
        return self.__resolved_str_cache__

    def evaluate(self, *, globals=None, locals=None, type_params=None, owner=None,
                 format=Format.VALUE):
        """Evaluate this reference in its defining or explicitly supplied namespaces."""
        if format == Format.STRING:
            return self.__resolved_str__
        if format not in (Format.VALUE, Format.FORWARDREF):
            raise NotImplementedError(format)

        cells = self.__cell__
        if cells is not None and not isinstance(cells, dict):
            try:
                return cells.cell_contents
            except ValueError:
                pass

        if owner is None:
            owner = self.__owner__
        if globals is None and self.__forward_module__ is not None:
            import sys
            globals = getattr(sys.modules.get(self.__forward_module__), "__dict__", None)
        if globals is None:
            globals = self.__globals__
        if globals is None and owner is not None:
            from types import ModuleType
            if isinstance(owner, type):
                import sys
                globals = getattr(sys.modules.get(owner.__module__), "__dict__", None)
            elif isinstance(owner, ModuleType):
                globals = owner.__dict__
            elif callable(owner):
                globals = getattr(owner, "__globals__", None)
        if globals is None:
            globals = {}

        if type_params is None and owner is not None:
            type_params = getattr(owner, "__type_params__", None)
        if locals is None:
            locals = dict(vars(owner)) if isinstance(owner, type) else {}
        elif type_params is not None or isinstance(cells, dict) or self.__extra_names__:
            locals = dict(locals)
        if type_params is not None:
            for param in type_params:
                locals.setdefault(param.__name__, param)
        if isinstance(cells, dict):
            for name, cell in cells.items():
                try:
                    value = cell.cell_contents
                except ValueError:
                    continue
                locals.setdefault(name, value)
        if self.__extra_names__:
            locals.update(self.__extra_names__)

        import typing
        arg = self.__forward_arg__
        source = f"({arg},)[0]" if arg.lstrip().startswith("*") else arg
        try:
            return typing._eval_type_expression(source, globals, locals)
        except SyntaxError:
            # Parsing fails before CPython enters its symbolic fallback. An
            # exception from the expression itself, however, still permits it.
            try:
                typing._validate_forward_ref(source)
            except SyntaxError:
                raise SyntaxError(f"Forward reference must be an expression -- got {arg!r}")
            if format == Format.VALUE:
                raise
        except NameError as error:
            if format == Format.VALUE:
                raise
            if error.name == arg:
                return self
        except Exception:
            if format == Format.VALUE:
                raise
        # For an unresolved compound, retry with missing names represented by
        # scoped forward references, preserving already-known parts of the result.
        import builtins
        names = {**vars(builtins), **globals, **locals}
        symbolic = typing._typeddict_names(
            globals, cells if isinstance(cells, dict) else None,
            owner, names, self.__forward_is_class__,
        )
        try:
            result = typing._eval_type_expression(source, globals, symbolic)
        except Exception:
            return self
        symbolic.finish()
        return result

    def _cell_identity(self):
        cells = self.__cell__
        if isinstance(cells, dict):
            return tuple(sorted((name, id(cell)) for name, cell in cells.items()))
        return id(cells)

    def __eq__(self, other):
        if not isinstance(other, ForwardRef):
            return NotImplemented
        return (
            self.__forward_arg__ == other.__forward_arg__
            and self.__forward_module__ == other.__forward_module__
            and self.__globals__ is other.__globals__
            and self._cell_identity() == other._cell_identity()
            and self.__forward_is_class__ == other.__forward_is_class__
            and self.__owner__ == other.__owner__
            and (self.__extra_names__ or {}) == (other.__extra_names__ or {})
        )

    def __hash__(self):
        return hash((
            self.__forward_arg__, self.__forward_module__,
            id(self.__globals__), self._cell_identity(), self.__forward_is_class__, self.__owner__,
            tuple(sorted(self.__extra_names__.items())) if self.__extra_names__ else None,
        ))

    def __repr__(self):
        extra = []
        if self.__forward_module__ is not None:
            extra.append(f", module={self.__forward_module__!r}")
        if self.__forward_is_class__:
            extra.append(", is_class=True")
        if self.__owner__ is not None:
            extra.append(f", owner={self.__owner__!r}")
        return f"ForwardRef({self.__resolved_str__!r}{''.join(extra)})"


def _get_dunder_annotations(obj):
    """Return the annotations for an object, checking that it is a dictionary.

    Does not return a fresh dictionary.
    """
    # This special case is needed to support types defined under
    # from __future__ import annotations, where accessing the __annotations__
    # attribute directly might return annotations for the wrong class.
    if isinstance(obj, type):
        from typing import _get_type_annotations
        try:
            ann = _get_type_annotations(obj)
        except AttributeError:
            # For static types, the descriptor raises AttributeError.
            return None
    else:
        ann = getattr(obj, "__annotations__", None)
        if ann is None:
            return None

    if not isinstance(ann, dict):
        raise ValueError(f"{obj!r}.__annotations__ is neither a dict nor None")
    return ann


def _rewrite_star_unpack(arg):
    """If the given argument annotation expression is a star unpack e.g. `'*Ts'`
       rewrite it to a valid expression.
       """
    if arg.lstrip().startswith("*"):
        return f"({arg},)[0]"  # E.g. (*Ts,)[0] or (*tuple[int, int],)[0]
    else:
        return arg


def _get_and_call_annotate(obj, format):
    """Get the __annotate__ function and call it.

    May not return a fresh dictionary.
    """
    annotate = getattr(obj, "__annotate__", None)
    if annotate is not None:
        from typing import _call_annotate
        ann = _call_annotate(annotate, format, owner=obj)
        if not isinstance(ann, dict):
            raise ValueError(f"{obj!r}.__annotate__ returned a non-dict")
        return ann
    return None


def get_annotations(
    obj, *, globals=None, locals=None, eval_str=False, format=Format.VALUE
):
    """Compute the annotations dict for an object.

    obj may be a callable, class, module, or other object with
    __annotate__ or __annotations__ attributes.
    Passing any other object raises TypeError.

    The *format* parameter controls the format in which annotations are returned,
    and must be a member of the Format enum or its integer equivalent.
    For the VALUE format, the __annotations__ is tried first; if it
    does not exist, the __annotate__ function is called. The
    FORWARDREF format uses __annotations__ if it exists and can be
    evaluated, and otherwise falls back to calling the __annotate__ function.
    The STRING format tries __annotate__ first, and falls back to
    using __annotations__, stringified using annotations_to_string().

    This function handles several details for you:

      * If eval_str is true, values of type str will
        be un-stringized using eval().  This is intended
        for use with stringized annotations
        ("from __future__ import annotations").
      * If obj doesn't have an annotations dict, returns an
        empty dict.  (Functions and methods always have an
        annotations dict; classes, modules, and other types of
        callables may not.)
      * Ignores inherited annotations on classes.  If a class
        doesn't have its own annotations dict, returns an empty dict.
      * All accesses to object members and dict values are done
        using getattr() and dict.get() for safety.
      * Always, always, always returns a freshly-created dict.

    eval_str controls whether or not values of type str are replaced
    with the result of calling eval() on those values:

      * If eval_str is true, eval() is called on values of type str.
      * If eval_str is false (the default), values of type str are unchanged.

    globals and locals are passed in to eval(); see the documentation
    for eval() for more information.  If either globals or locals is
    None, this function may replace that value with a context-specific
    default, contingent on type(obj):

      * If obj is a module, globals defaults to obj.__dict__.
      * If obj is a class, globals defaults to
        sys.modules[obj.__module__].__dict__ and locals
        defaults to the obj class namespace.
      * If obj is a callable, globals defaults to obj.__globals__,
        although if obj is a wrapped function (using
        functools.update_wrapper()) it is first unwrapped.
    """
    if eval_str and format != Format.VALUE:
        raise ValueError("eval_str=True is only supported with format=Format.VALUE")

    match format:
        case Format.VALUE:
            # For VALUE, we first look at __annotations__
            ann = _get_dunder_annotations(obj)

            # If it's not there, try __annotate__ instead
            if ann is None:
                ann = _get_and_call_annotate(obj, format)
        case Format.FORWARDREF:
            # For FORWARDREF, we use __annotations__ if it exists
            try:
                ann = _get_dunder_annotations(obj)
            except Exception:
                pass
            else:
                if ann is not None:
                    return dict(ann)

            # But if __annotations__ threw a NameError, we try calling __annotate__
            ann = _get_and_call_annotate(obj, format)
            if ann is None:
                # If that didn't work either, we have a very weird object: evaluating
                # __annotations__ threw NameError and there is no __annotate__. In that case,
                # we fall back to trying __annotations__ again.
                ann = _get_dunder_annotations(obj)
        case Format.STRING:
            # For STRING, we try to call __annotate__
            ann = _get_and_call_annotate(obj, format)
            if ann is not None:
                return dict(ann)
            # But if we didn't get it, we use __annotations__ instead.
            ann = _get_dunder_annotations(obj)
            if ann is not None:
                return annotations_to_string(ann)
        case Format.VALUE_WITH_FAKE_GLOBALS:
            raise ValueError("The VALUE_WITH_FAKE_GLOBALS format is for internal use only")
        case _:
            raise ValueError(f"Unsupported format {format!r}")

    if ann is None:
        if isinstance(obj, type) or callable(obj):
            return {}
        raise TypeError(f"{obj!r} does not have annotations")

    if not ann:
        return {}

    if not eval_str:
        return dict(ann)

    if globals is None or locals is None:
        if isinstance(obj, type):
            # class
            obj_globals = None
            module_name = getattr(obj, "__module__", None)
            if module_name:
                module = sys.modules.get(module_name, None)
                if module:
                    obj_globals = getattr(module, "__dict__", None)
            obj_locals = dict(vars(obj))
            unwrap = obj
        elif isinstance(obj, types.ModuleType):
            # module
            obj_globals = getattr(obj, "__dict__")
            obj_locals = None
            unwrap = None
        elif callable(obj):
            # this includes types.Function, types.BuiltinFunctionType,
            # types.BuiltinMethodType, functools.partial, functools.singledispatch,
            # "class funclike" from Lib/test/test_inspect... on and on it goes.
            obj_globals = getattr(obj, "__globals__", None)
            obj_locals = None
            unwrap = obj
        else:
            obj_globals = obj_locals = unwrap = None

        if unwrap is not None:
            # Use an id-based visited set to detect cycles in the __wrapped__
            # and functools.partial.func chain (e.g. f.__wrapped__ = f).
            # On cycle detection we stop and use whatever __globals__ we have
            # found so far, mirroring the approach of inspect.unwrap().
            _seen_ids = {id(unwrap)}
            while True:
                if hasattr(unwrap, "__wrapped__"):
                    candidate = unwrap.__wrapped__
                    if id(candidate) in _seen_ids:
                        break
                    _seen_ids.add(id(candidate))
                    unwrap = candidate
                    continue
                if functools := sys.modules.get("functools"):
                    if isinstance(unwrap, functools.partial):
                        candidate = unwrap.func
                        if id(candidate) in _seen_ids:
                            break
                        _seen_ids.add(id(candidate))
                        unwrap = candidate
                        continue
                break
            if hasattr(unwrap, "__globals__"):
                obj_globals = unwrap.__globals__

        if globals is None:
            globals = obj_globals
        if locals is None:
            locals = obj_locals

    # "Inject" type parameters into the local namespace
    # (unless they are shadowed by assignments *in* the local namespace),
    # as a way of emulating annotation scopes when calling `eval()`
    if type_params := getattr(obj, "__type_params__", ()):
        if locals is None:
            locals = {}
        locals = {param.__name__: param for param in type_params} | locals

    return_value = {
        key: value if not isinstance(value, str)
        else eval(_rewrite_star_unpack(value), globals, locals)
        for key, value in ann.items()
    }
    return return_value


_get_value_annotations = get_annotations
