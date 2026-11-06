from _imre_importlib import namespace_locations as _namespace_locations

_ModuleSpec = _bootstrap.ModuleSpec


def _file_spec(name, origin, locations, loader=None):
    if loader is None:
        loader = _bootstrap_external.SourceFileLoader(name, origin)
    spec = _ModuleSpec(name, loader, origin=origin)
    spec.submodule_search_locations = locations
    spec.has_location = True
    return spec


def _find_namespace_portions(name, path):
    locations = _namespace_locations(name, path)
    if locations is not None:
        spec = _ModuleSpec(name, None)
        spec.submodule_search_locations = locations
        return spec


def _namespace_spec(name, locations, module, source_globals):
    # The installed finder supplies the spec and path. The importing core installs the loader.
    finder = _bootstrap_external._finder_external
    spec = finder._bootstrap.ModuleSpec(name, None)
    spec.submodule_search_locations = finder._NamespacePath(name, locations, _find_namespace_portions)
    if module is not None:
        init_attrs = (finder._bootstrap._init_module_attrs if source_globals is None
                      else source_globals["_init_module_attrs"])
        init_attrs(spec, module, override=True)
        spec._initializing = False
    return spec
