import _lzma
import gc
from _lzma import *


def current():
    import lzma
    from lzma import LZMAError

    assert LZMAError is _lzma.LZMAError
    # XZ and legacy LZMA-alone streams compressed with preset 1.
    xz = bytes.fromhex(
        "fd377a585a000004e6d6b4460200210110000000a8708e86e1d4bf005d5d0024"
        "9b4a46637dff5d53db48bc629064ce8a107a2265cb621cabd9dc5ef1ef8164"
        "2752e6210b23e9f362c73feabeaa4b36daece26db0ea43055ec6c3ce71bce5"
        "53d3e059f37683b10fe26aa74c82abea4fa308afca330c83c6f92e345600000"
        "000001e9aaf5166833e32000179c0a907000005204133b1c467fb020000000004595a"
    )
    alone = bytes.fromhex("5d00001000ffffffffffffffff0039994891b169953d46a4fffffe73b000")
    expected = b"Imre\x00\xff" * 20000
    assert lzma.decompress(xz) == expected
    assert lzma.decompress(alone) == b"second\x00"
    assert lzma.decompress(xz + alone) == expected + b"second\x00"
    assert lzma.decompress(xz + b"no valid header here") == expected
    assert lzma.decompress(xz, memlimit=16 << 20) == expected
    print("XZ, LZMA-alone, concatenation, trailing data and memory limit passed")

    # Flip one byte in the XZ integrity check.
    corrupt = xz[:-26] + bytes([xz[-26] ^ 1]) + xz[-25:]
    truncated = "Compressed data ended before the end-of-stream marker was reached"
    for label, stream, limit, message in (
        ("corrupt", corrupt, None, "Corrupt input data"),
        ("truncated", xz[:-1], None, truncated),
        ("truncated second stream", xz + alone[:-1], None, truncated),
        ("memory limit", xz, 4096, "Memory usage limit exceeded"),
    ):
        try:
            lzma.decompress(stream, memlimit=limit)
        except LZMAError as error:
            assert type(error) is lzma.LZMAError
            assert str(error) == message
            print(label, str(error))
        else:
            raise AssertionError(label)

    extension_stream()


def mutable_filter_options():
    lookups = []

    class Spec(dict):
        def __getitem__(self, name):
            lookups.append(name)
            return dict.__getitem__(self, name)

    class FilterId:
        def __index__(self):
            spec["preset"] = 0
            gc.collect()
            return _lzma.FILTER_LZMA1

    class DictionarySize:
        def __index__(self):
            spec["lc"] = 1
            gc.collect()
            return 65536

    # Both old values are owned only by the spec. The field parser reads
    # dict_size before lc even though they were inserted in the other order.
    spec = Spec(id=FilterId(), preset=object(), lc=object(), dict_size=DictionarySize())
    assert _lzma._encode_filter_properties(spec) == bytes.fromhex("5b00000100")
    assert lookups == ["id", "preset"]


def raw_filter_sequences():
    delta = {"id": FILTER_DELTA, "dist": 2}
    lzma2 = {"id": FILTER_LZMA2, "dict_size": 1 << 16}
    payload = b"stream with two filters" * 32
    events = []

    class Sequence:
        def __init__(self, items):
            self.items = items

        def __len__(self):
            events.append("len")
            return len(self.items)

        def __getitem__(self, index):
            events.append(index)
            return self.items[index]

        def keys(self):
            raise AssertionError("keys() used for a filter chain")

        def get(self, key, default=None):
            raise AssertionError("get() used for a filter chain")

    class InheritedDict(dict):
        pass

    class DictSequence(dict):
        def __len__(self):
            events.append("len")
            return dict.__len__(self)

        def __getitem__(self, index):
            events.append(index)
            return dict.__getitem__(self, index)

    class MutatingList(list):
        def __getitem__(self, index):
            events.append(index)
            if index == 0:
                self[1] = lzma2
                gc.collect()
            return list.__getitem__(self, index)

    for container, items, expected in (
        (list, [delta, lzma2], []),
        (tuple, [delta, lzma2], []),
        (Sequence, [delta, lzma2], ["len", 0, 1]),
        (InheritedDict, {0: delta, 1: lzma2}, []),
        (DictSequence, {0: delta, 1: lzma2}, ["len", 0, 1]),
        (MutatingList, [delta, None], [0, 1]),
    ):
        events.clear()
        encoder = LZMACompressor(format=FORMAT_RAW, filters=container(items))
        compressed = encoder.compress(payload) + encoder.flush()
        decoder = LZMADecompressor(format=FORMAT_RAW, filters=container(items))
        assert decoder.decompress(compressed) == payload and decoder.eof
        assert events == expected * 2

    for constructor in (LZMACompressor, LZMADecompressor):
        try:
            constructor(format=FORMAT_RAW, filters={0: lzma2})
        except TypeError as error:
            assert str(error) == "dict is not a sequence"
        else:
            raise AssertionError("accepted a plain dictionary as a filter chain")


def compressor_buffer_release():
    observations = []

    class Input(bytearray):
        def __release_buffer__(self, view):
            observations.append((view.obj is None, view.tobytes(), view.readonly))

    data = Input(b"abc")
    encoder = LZMACompressor()
    stream = encoder.compress(data)
    assert observations == [(True, b"abc", False)]
    data.append(100)
    assert LZMADecompressor().decompress(stream + encoder.flush()) == b"abc"
    try:
        encoder.compress(data)
    except ValueError as error:
        assert str(error) == "Compressor has been flushed"
    else:
        raise AssertionError("accepted input after flush")
    assert observations == [(True, b"abc", False), (True, b"abcd", False)]


def extension_stream():
    from _lzma import _decode_filter_properties, _encode_filter_properties

    # CPython's lzma.py takes the public names via * and imports these helpers explicitly.
    exports = (
        "CHECK_NONE CHECK_CRC32 CHECK_CRC64 CHECK_SHA256 CHECK_ID_MAX CHECK_UNKNOWN "
        "FILTER_LZMA1 FILTER_LZMA2 FILTER_DELTA FILTER_X86 FILTER_IA64 FILTER_ARM "
        "FILTER_ARMTHUMB FILTER_POWERPC FILTER_SPARC FORMAT_AUTO FORMAT_XZ FORMAT_ALONE "
        "FORMAT_RAW MF_HC3 MF_HC4 MF_BT2 MF_BT3 MF_BT4 MODE_FAST MODE_NORMAL "
        "PRESET_DEFAULT PRESET_EXTREME LZMACompressor LZMADecompressor LZMAError is_check_supported"
    ).split()
    for name in exports:
        assert globals()[name] is getattr(_lzma, name)
    assert "_encode_filter_properties" not in globals() and "_decode_filter_properties" not in globals()
    assert is_check_supported(CHECK_NONE) is True
    assert is_check_supported(CHECK_CRC32) is True
    assert is_check_supported(CHECK_ID_MAX + 1) is False
    for cls in (LZMACompressor, LZMADecompressor, LZMAError):
        assert cls.__module__ == "_lzma"
    mutable_filter_options()
    raw_filter_sequences()

    default = {"id": FILTER_LZMA1}
    assert _encode_filter_properties(default) == bytes.fromhex("5d00008000")
    settings = dict(default, dict_size=1 << 17, lc=2, lp=1, pb=1)
    props = _encode_filter_properties(settings)
    assert props == bytes.fromhex("3800000200")
    assert _decode_filter_properties(FILTER_LZMA1, props) == settings

    encoder = _lzma.LZMACompressor(format=FORMAT_RAW, filters=[settings])
    payload = b"abcdef" * 512
    data = encoder.compress(payload[:19]) + encoder.compress(payload[19:]) + encoder.flush()
    decoder = _lzma.LZMADecompressor(format=FORMAT_RAW, filters=[settings])
    assert type(encoder) is LZMACompressor and type(decoder) is LZMADecompressor
    assert decoder.needs_input and not decoder.eof and decoder.unused_data == b""
    assert decoder.decompress(data[:1]) == b"" and decoder.needs_input
    assert decoder.decompress(data[1:] + b"tail", max_length=11) == payload[:11]
    assert not decoder.eof and not decoder.needs_input
    assert decoder.decompress(b"") == payload[11:] and decoder.eof
    gc.collect()
    tail = decoder.unused_data
    assert tail == b"tail" and decoder.unused_data is tail
    compressor_buffer_release()

    for error_type, operation, args in (
        (ValueError, encoder.compress, (b"",)),
        (ValueError, encoder.flush, ()),
        (EOFError, decoder.decompress, (b"",)),
        (LZMAError, _decode_filter_properties, (FILTER_LZMA1, b"\xff" * 5)),
    ):
        try:
            operation(*args)
        except (ValueError, EOFError, LZMAError) as error:
            assert type(error) is error_type
        else:
            raise AssertionError("unexpected success")


if __name__ == "__main__":
    current()
