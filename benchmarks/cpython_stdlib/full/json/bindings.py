"""Bind upstream JSON fixtures to native operations without masking transport failures."""


class Boundary(BaseException):
    """A transport limitation must never satisfy an upstream assertRaises."""


class Missing:
    def __init__(self, path):
        self.path = path

    def __getattr__(self, name):
        raise Boundary(f"No native driver binding for {self.path}.{name}")


class Probe:
    def __init__(self, native):
        self.native = native
        self.encoder, self.decoder, self.scanner = (
            Missing("json." + name) for name in ("encoder", "decoder", "scanner"))

    def __getattr__(self, name):
        if name not in ("dump", "dumps", "load", "loads"):
            raise Boundary(f"No native driver binding for json.{name}")
        operation = getattr(self.native, name)

        def call(*args, **kwargs):
            before = len(self.native.calls)
            try:
                return operation(*args, **kwargs)
            except Exception as exc:
                calls = self.native.calls[before:]
                if len(calls) == 1 and calls[0]["status"] == type(exc).__name__:
                    raise
                if len(calls) == 1 and calls[0]["status"] == "UnicodeDecodeError" and isinstance(exc, UnicodeDecodeError):
                    raise
                # Repr transport can itself fail to reconstruct e.g. a 5,001-digit
                # integer. Such a host ValueError must not pass a native exception test.
                detail = str(exc).replace("Native.", "").splitlines()[0]
                raise Boundary(f"json.{name} bridge: {type(exc).__name__}: {detail}") from None
        return call


# These sources assert CPython's backend identity or call a private accelerator factory.
# Three tests in test_speedups instead call public classes and must be attempted.
PRIVATE = {
    "TestCTest.test_cjson": "requires json scanner/encoder/decoder objects defined in _json",
    "TestPyTest.test_pyjson": "requires decoder/encoder/scanner objects defined in pure CPython modules",
    "TestSpeedups.test_scanstring": "asserts _json.scanstring identity and __module__",
    "TestSpeedups.test_encode_basestring_ascii": "asserts _json.encode_basestring_ascii identity and __module__",
    "TestDecode.test_make_scanner": "directly calls the private c_make_scanner factory",
    "TestEncode.test_make_encoder": "directly calls the private c_make_encoder factory",
    "TestEncode.test_bad_str_encoder": "injects callbacks into the private c_make_encoder factory",
    "TestEncode.test_bad_markers_argument_to_encoder": "tests the private c_make_encoder signature",
    "TestEncode.test_current_indent_level": "calls a private c_make_encoder result with its internal indent level",
}
