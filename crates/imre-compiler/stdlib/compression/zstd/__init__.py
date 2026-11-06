"""Zstandard compression and file I/O."""

import _zstd
from _zstd import ZstdCompressor, ZstdDecompressor, ZstdError, zstd_version
from compression.zstd._zstdfile import ZstdFile, open

__all__ = (
    'COMPRESSION_LEVEL_DEFAULT', 'compress', 'decompress', 'open',
    'ZstdCompressor', 'ZstdDecompressor', 'ZstdError', 'ZstdFile',
    'zstd_version', 'zstd_version_info',
)

COMPRESSION_LEVEL_DEFAULT = _zstd.ZSTD_CLEVEL_DEFAULT
zstd_version_info = (*divmod(_zstd.zstd_version_number // 100, 100),
                     _zstd.zstd_version_number % 100)


def compress(data, level=None, options=None, zstd_dict=None):
    """Return Zstandard compressed *data* as bytes."""
    comp = ZstdCompressor(level=level, options=options, zstd_dict=zstd_dict)
    return comp.compress(data, mode=ZstdCompressor.FLUSH_FRAME)


def decompress(data, zstd_dict=None, options=None):
    """Decompress one or more frames of Zstandard compressed *data*."""
    results = []
    while True:
        decomp = ZstdDecompressor(options=options, zstd_dict=zstd_dict)
        results.append(decomp.decompress(data))
        if not decomp.eof:
            raise ZstdError('Compressed data ended before the '
                            'end-of-stream marker was reached')
        data = decomp.unused_data
        if not data:
            break
    return b''.join(results)
