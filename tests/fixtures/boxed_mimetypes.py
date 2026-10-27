import mimetypes


def current():
    print(mimetypes.inited, mimetypes._db is None)
    print(mimetypes.suffix_map[".tgz"], mimetypes.encodings_map[".gz"])
    print(mimetypes.types_map[".html"], mimetypes.common_types[".jpg"])
    print(len(mimetypes.types_map), len(mimetypes.common_types))
    knownfiles = mimetypes.knownfiles
    mimetypes.knownfiles = []
    previous = mimetypes.types_map
    previous[".imre-test"] = "application/x-imre"
    mimetypes._default_mime_types()
    print(previous[".imre-test"], ".imre-test" in mimetypes.types_map)
    print(mimetypes.types_map is mimetypes._types_map_default, mimetypes.knownfiles)
    mimetypes.knownfiles = knownfiles
    print(mimetypes.knownfiles is knownfiles)
