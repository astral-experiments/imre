"""Annotation formats and forward reference values from CPython 3.14.5 (PSF-2.0)."""

import enum

__all__ = ["Format", "ForwardRef"]


class Format(enum.IntEnum):
    VALUE = 1
    VALUE_WITH_FAKE_GLOBALS = 2
    FORWARDREF = 3
    STRING = 4


def _make_forward_ref(arg, module=None, owner=None, *, is_argument=True, is_class=False):
    from _typing import _validate_forward_ref
    # typing._make_forward_ref eagerly checks the syntax, without resolving names.
    source = f"({arg},)[0]" if arg.lstrip().startswith("*") else arg
    try:
        _validate_forward_ref(source)
    except SyntaxError:
        raise SyntaxError(f"Forward reference must be an expression -- got {arg!r}")
    return ForwardRef(arg, module=module, owner=owner, is_argument=is_argument, is_class=is_class)


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
                from typing import _annotation_type_repr
                rendered = "".join(
                    part if type(part) is str else _annotation_type_repr(part[1])
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

        from _typing import _eval_type_expression, _validate_forward_ref
        arg = self.__forward_arg__
        source = f"({arg},)[0]" if arg.lstrip().startswith("*") else arg
        try:
            return _eval_type_expression(source, globals, locals)
        except SyntaxError:
            # Parsing fails before CPython enters its symbolic fallback. An
            # exception from the expression itself, however, still permits it.
            try:
                _validate_forward_ref(source)
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
        from typing import _typeddict_names
        symbolic = _typeddict_names(
            globals, cells if isinstance(cells, dict) else None,
            owner, names, self.__forward_is_class__,
        )
        try:
            result = _eval_type_expression(source, globals, symbolic)
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


def _get_value_annotations(obj):
    annotations = None
    if isinstance(obj, type):
        namespace = getattr(obj, '__dict__', {})
        if ('__annotations__' in namespace or '__annotations_cache__' in namespace
                or '__annotate_func__' in namespace):
            annotations = getattr(obj, '__annotations__', None)
    else:
        annotations = getattr(obj, '__annotations__', None)
    if annotations is None:
        annotate = getattr(obj, '__annotate__', None)
        if annotate is not None:
            annotations = annotate(1)
            if not isinstance(annotations, dict):
                raise ValueError(f"{obj!r}.__annotate__ returned a non-dict")
        elif isinstance(obj, type) or callable(obj):
            return {}
        else:
            raise TypeError(f"{obj!r} does not have annotations")
    if not isinstance(annotations, dict):
        raise ValueError(f"{obj!r}.__annotations__ is neither a dict nor None")
    return dict(annotations)
