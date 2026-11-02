import io
import struct


def current():
    le = b'\xff\xfeA\0=\xd8\x00\xde\r\0\n\0\xff\xfeZ\0'
    be = b'\xfe\xff\0A\xd8=\xde\x00\0\r\0\n\xfe\xff\0Z'
    for raw in (le, be):
        for chunk in (1, 2, 3, 8192):
            stream = io.TextIOWrapper(io.BytesIO(raw), encoding='utf-16')
            stream._CHUNK_SIZE = chunk
            assert stream.read(1) == 'A'
            pos = stream.tell()
            assert stream.readline() == '😀\n'
            assert stream.read() == '\ufeffZ'
            assert stream.seek(pos) == pos
            assert stream.read() == '😀\n\ufeffZ'
            assert stream.seek(0) == 0
            assert stream.read() == 'A😀\n\ufeffZ'
        for spelling in ('utf-16-le', 'utf-16-be'):
            sample = le if spelling.endswith('le') else be
            stream = io.TextIOWrapper(io.BytesIO(sample), encoding=spelling)
            assert stream.read() == '\ufeffA😀\n\ufeffZ'
    for read in ('read', 'readline', 'bounded'):
        for raw in (b'', b'\xff', b'A\0', b'\xff\xfe\xff'):
            stream = io.TextIOWrapper(io.BytesIO(raw), encoding='utf-16')
            stream._CHUNK_SIZE = 1
            try:
                result = stream.read(1) if read == 'bounded' else getattr(stream, read)()
            except UnicodeDecodeError as exc:
                if raw == b'A\0':
                    assert (exc.encoding, exc.start, exc.end, exc.reason) == (
                        'utf-16', 0, 2, 'Stream does not start with BOM')
                else:
                    assert exc.reason == 'truncated data'
            else:
                assert raw == b'' and result == ''
    for enc in ('utf-32-le', 'utf-32-be'):
        raw = 'A😀\r\nB'.encode(enc)
        stream = io.TextIOWrapper(io.BytesIO(raw), encoding=enc)
        stream._CHUNK_SIZE = 1
        assert stream.read(1) == 'A'
        position = stream.tell()
        assert stream.readline() == '😀\n'
        stream.seek(position)
        assert stream.read() == '😀\nB'
        stream = io.TextIOWrapper(io.BytesIO(raw[:-1]), encoding=enc)
        try:
            stream.read()
        except UnicodeDecodeError as exc:
            assert (exc.encoding, exc.start, exc.end, exc.reason) == (enc, len(raw) - 4, len(raw) - 1, 'truncated data')
        else:
            raise AssertionError('partial UTF-32 code unit accepted')

    # Retain the saved byte order when iteration reads a later chunk.
    fp = io.TextIOWrapper(io.BytesIO(b'\xfe\xff\0A\0\n\0B\0\n\0C\0\n\0D\0\n'), 'utf-16')
    fp._CHUNK_SIZE = 6
    assert fp.read(1) == 'A' and next(fp) == '\n' and next(fp) == 'B\n'
    fp.flush()
    cookie = fp.tell()
    assert fp.seek(cookie) == cookie and fp.read() == '\nD\n'
    output = io.BytesIO()
    writer = io.TextIOWrapper(output, encoding='utf-16')
    writer.write('X')
    writer.write('Y')
    writer.flush()
    expected = b'\xff\xfeX\0Y\0' if struct.pack('H', 1) == b'\1\0' else b'\xfe\xff\0X\0Y'
    assert output.getvalue() == expected

    output = io.BytesIO()
    writer = io.TextIOWrapper(output, encoding='utf-16')
    try:
        writer.write(chr(0xd800))
    except UnicodeEncodeError as exc:
        assert exc.encoding == 'utf-16'
    else:
        raise AssertionError('strict UTF-16 accepted an unpaired surrogate')
    writer.write('X')
    writer.flush()
    assert output.getvalue() == expected[2:4]


def native_utf32():
    for enc in ('utf-32-le', 'utf-32-be'):
        raw = '\ufeffA😀\r\n\ufeffZ'.encode(enc)
        stream = io.TextIOWrapper(io.BytesIO(raw), encoding='UTF32')
        stream._CHUNK_SIZE = 1
        assert stream.read(1) == 'A'
        cookie = stream.tell()
        assert stream.readline() == '😀\n'
        assert stream.seek(cookie) == cookie and stream.read() == '😀\n\ufeffZ'
        assert stream.seek(0) == 0 and stream.read() == 'A😀\n\ufeffZ'

    stream = io.TextIOWrapper(io.BytesIO(), encoding='u32')
    assert stream.readline() == '' and stream.tell() == 0
    stream = io.TextIOWrapper(io.BytesIO(b'\xff'), encoding='utf-32', errors='replace')
    assert stream.readline() == '\ufffd'
    stream.reconfigure(encoding='ascii')
    assert stream.tell() == 1
    for data, expected in ((b'\0' * 4, 'Stream does not start with BOM'),
                           (b'\0', 'truncated data')):
        stream = io.TextIOWrapper(io.BytesIO(data), encoding='utf-32')
        stream._CHUNK_SIZE = 1
        try:
            stream.read(1)
        except UnicodeDecodeError as exc:
            assert (exc.start, exc.end, exc.reason) == (0, len(data), expected)
        else:
            raise AssertionError('bad UTF-32 input accepted')

    class Pipe(io.BytesIO):
        def seekable(self):
            return False

    for raw, bom in ((io.BytesIO(), True), (Pipe(), False)):
        stream = io.TextIOWrapper(raw, encoding='utf-32')
        assert stream.write('') == 0
        stream.flush()
        assert raw.getvalue() == (''.encode('utf-32') if bom else b'')
        stream.write('A')
        stream.reconfigure(encoding='utf-32')
        stream.write('B')
        stream.flush()
        expected = 'AB'.encode('utf-32')
        assert raw.getvalue() == (expected if bom else expected[4:])
        if bom:
            stream = io.TextIOWrapper(stream.detach(), encoding='utf-32')
            stream.write('C')
            stream.flush()
            assert raw.getvalue() == 'ABC'.encode('utf-32')
            stream.seek(0)
            stream.write('Z')
            stream.flush()
            assert raw.getvalue() == 'ZBC'.encode('utf-32')


if __name__ == '__main__':
    current()
    native_utf32()
