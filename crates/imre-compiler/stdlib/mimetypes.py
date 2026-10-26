"""Filename and URL MIME lookup using the CPython 3.14.5 registry (PSF License)."""
from __future__ import annotations

import codecs
import posixpath
from pathlib import Path
from typing import TextIO
from urllib.parse import urlsplit


class _Registry:
    suffix_map: dict[str, str]
    encodings_map: dict[str, str]
    types_map: tuple[dict[str, str], dict[str, str]]
    types_map_inv: tuple[dict[str, list[str]], dict[str, list[str]]]

    def __init__(self, filenames: list[str] | None = None, strict: bool = True) -> None:
        self.suffix_map = dict(_state.suffix_defaults)
        self.encodings_map = dict(_state.encoding_defaults)
        self.types_map = ({}, {})
        self.types_map_inv = ({}, {})
        _populate(self, filenames, strict)

    def add_type(self, type: str, ext: str, strict: bool = True) -> None:
        if ext and not ext.startswith('.'):
            raise NotImplementedError('undotted extensions require a DeprecationWarning')
        if not type:
            return
        forward = self.types_map[1] if strict else self.types_map[0]
        inverse = self.types_map_inv[1] if strict else self.types_map_inv[0]
        forward[ext] = type
        suffixes = inverse.setdefault(type, [])
        if ext not in suffixes:
            suffixes.append(ext)

    def guess_type(self, url: str | bytes | Path, strict: bool = True) -> tuple[str | None, str | None]:
        if isinstance(url, bytes):
            # urlparse first decodes bytes as ASCII. Its bytes suffix cannot match a text key.
            if len(urlsplit(url).scheme) > 1:
                return None, None
            return self.guess_file_type(url, strict=strict)
        text = _filename(url)
        parsed = urlsplit(text)
        scheme = parsed.scheme
        if len(scheme) <= 1:
            return self._guess(text, strict)
        path = parsed.path
        if scheme == 'data':
            comma = path.find(',')
            if comma < 0:
                return None, None
            mime = path[:comma].split(';')[0]
            if '=' in mime or '/' not in mime:
                mime = 'text/plain'
            return mime, None
        # urlparse separates parameters from the last segment only for these schemes.
        if scheme in _uses_params:
            index = path.find(';', len(path) - len(path.rsplit('/', 1)[-1]))
            if index >= 0:
                path = path[:index]
        return self._guess(path, strict)

    def guess_file_type(self, path: str | bytes | Path, *, strict: bool = True) -> tuple[str | None, str | None]:
        return self._guess(_filename(path), strict)

    def _guess(self, path: str, strict: bool) -> tuple[str | None, str | None]:
        base, ext = posixpath.splitext(path)
        while ext.lower() in self.suffix_map:
            base, ext = posixpath.splitext(base + self.suffix_map[ext.lower()])
        encoding = self.encodings_map.get(ext)
        if encoding is not None:
            base, ext = posixpath.splitext(base)
        ext = ext.lower()
        mime = self.types_map[1].get(ext)
        if mime is None and not strict:
            mime = self.types_map[0].get(ext)
        return mime, encoding

    def guess_all_extensions(self, type: str, strict: bool = True) -> list[str]:
        mime = type.lower()
        extensions = list(self.types_map_inv[1].get(mime, []))
        if not strict:
            for ext in self.types_map_inv[0].get(mime, []):
                if ext not in extensions:
                    extensions.append(ext)
        return extensions

    def guess_extension(self, type: str, strict: bool = True) -> str | None:
        extensions = self.guess_all_extensions(type, strict)
        if extensions:
            return extensions[0]
        return None

    def readfp(self, fp: TextIO, strict: bool = True) -> None:
        while line := fp.readline():
            words = line.split()
            if words and not words[0].startswith('#'):
                for extension in words[1:]:
                    if extension.startswith('#'):
                        break
                    self.add_type(words[0], '.' + extension, strict)

    def read(self, filename: str, strict: bool = True) -> None:
        with open(filename, encoding='utf-8') as fp:
            self.readfp(fp, strict)


def _filename(path: str | bytes | Path) -> str:
    if isinstance(path, str):
        return path
    if isinstance(path, bytes):
        return codecs.lookup('utf-8').decode(path, 'surrogateescape')[0]
    if isinstance(path, Path):
        return str(path)
    raise TypeError('expected str, bytes or pathlib.Path')


def _populate(db: _Registry, filenames: list[str] | None, strict: bool) -> None:
    for ext, mime in _state.standard_defaults.items():
        db.add_type(mime, ext)
    for ext, mime in _state.common_defaults.items():
        db.add_type(mime, ext, False)
    if filenames is not None:
        for filename in filenames:
            db.read(filename, strict)


class MimeTypes(_Registry):
    def __init__(self, filenames: list[str] | None = None, strict: bool = True) -> None:
        _ = _ensure_inited()
        super().__init__(filenames, strict)


def _ensure_inited() -> None:
    if not _state.inited:
        init()


class _State:
    inited: bool
    db: _Registry | None
    knownfiles: list[str]
    suffix_defaults: dict[str, str]
    encoding_defaults: dict[str, str]
    standard_defaults: dict[str, str]
    common_defaults: dict[str, str]
    suffix_map: dict[str, str]
    encodings_map: dict[str, str]
    types_map: dict[str, str]
    common_types: dict[str, str]

    def __init__(self) -> None:
        self.inited = False
        self.db = None
        self.knownfiles = _knownfiles()
        self.suffix_defaults = _suffixes()
        self.encoding_defaults = _encodings()
        self.standard_defaults = _standard()
        self.common_defaults = _common()
        self.suffix_map = self.suffix_defaults
        self.encodings_map = self.encoding_defaults
        self.types_map = self.standard_defaults
        self.common_types = self.common_defaults


def _default_mime_types() -> None:
    _state.suffix_defaults = _suffixes()
    _state.encoding_defaults = _encodings()
    _state.standard_defaults = _standard()
    _state.common_defaults = _common()
    _state.suffix_map = _state.suffix_defaults
    _state.encodings_map = _state.encoding_defaults
    _state.types_map = _state.standard_defaults
    _state.common_types = _state.common_defaults


def init(files: list[str] | None = None) -> None:
    _state.inited = True
    db = _state.db
    if files is None or db is None:
        db = _Registry()
        for filename in _state.knownfiles:
            if Path(filename).is_file():
                db.read(filename)
    if files is not None:
        for filename in files:
            if Path(filename).is_file():
                db.read(filename)
    _state.suffix_map = db.suffix_map
    _state.encodings_map = db.encodings_map
    _state.types_map = db.types_map[1]
    _state.common_types = db.types_map[0]
    _state.db = db


def _database() -> _Registry:
    if _state.db is None:
        init()
    db = _state.db
    assert db is not None
    return db


def guess_type(url: str | bytes | Path, strict: bool = True) -> tuple[str | None, str | None]:
    return _database().guess_type(url, strict)


def guess_file_type(path: str | bytes | Path, *, strict: bool = True) -> tuple[str | None, str | None]:
    return _database().guess_file_type(path, strict=strict)


def guess_all_extensions(type: str, strict: bool = True) -> list[str]:
    return _database().guess_all_extensions(type, strict)


def guess_extension(type: str, strict: bool = True) -> str | None:
    return _database().guess_extension(type, strict)


def add_type(type: str, ext: str, strict: bool = True) -> None:
    _database().add_type(type, ext, strict)


def read_mime_types(file: str) -> dict[str, str] | None:
    try:
        fp = open(file, encoding='utf-8')
    except OSError:
        return None
    with fp:
        db = MimeTypes()
        db.readfp(fp, True)
        return db.types_map[1]


def _knownfiles() -> list[str]:
    return [
    "/etc/mime.types",
    "/etc/httpd/mime.types",                    # Mac OS X
    "/etc/httpd/conf/mime.types",               # Apache
    "/etc/apache/mime.types",                   # Apache 1
    "/etc/apache2/mime.types",                  # Apache 2
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",     # Apache 1.2
    "/usr/local/etc/mime.types",                # Apache 1.3
    ]

_uses_params = ['', 'ftp', 'hdl', 'prospero', 'http', 'imap', 'https', 'shttp', 'rtsp', 'rtsps', 'rtspu', 'sip', 'sips', 'mms', 'sftp', 'tel']


def _suffixes() -> dict[str, str]:
    return {
        '.svgz': '.svg.gz',
        '.tgz': '.tar.gz',
        '.taz': '.tar.gz',
        '.tz': '.tar.gz',
        '.tbz2': '.tar.bz2',
        '.txz': '.tar.xz',
        }


def _encodings() -> dict[str, str]:
    return {
        '.gz': 'gzip',
        '.Z': 'compress',
        '.bz2': 'bzip2',
        '.xz': 'xz',
        '.br': 'br',
        }


def _standard() -> dict[str, str]:
    return {
        '.js'     : 'text/javascript',
        '.mjs'    : 'text/javascript',
        '.epub'   : 'application/epub+zip',
        '.gz'     : 'application/gzip',
        '.json'   : 'application/json',
        '.webmanifest': 'application/manifest+json',
        '.doc'    : 'application/msword',
        '.dot'    : 'application/msword',
        '.wiz'    : 'application/msword',
        '.nq'     : 'application/n-quads',
        '.nt'     : 'application/n-triples',
        '.bin'    : 'application/octet-stream',
        '.a'      : 'application/octet-stream',
        '.dll'    : 'application/octet-stream',
        '.exe'    : 'application/octet-stream',
        '.o'      : 'application/octet-stream',
        '.obj'    : 'application/octet-stream',
        '.so'     : 'application/octet-stream',
        '.oda'    : 'application/oda',
        '.ogx'    : 'application/ogg',
        '.pdf'    : 'application/pdf',
        '.p7c'    : 'application/pkcs7-mime',
        '.ps'     : 'application/postscript',
        '.ai'     : 'application/postscript',
        '.eps'    : 'application/postscript',
        '.trig'   : 'application/trig',
        '.m3u'    : 'application/vnd.apple.mpegurl',
        '.m3u8'   : 'application/vnd.apple.mpegurl',
        '.xls'    : 'application/vnd.ms-excel',
        '.xlb'    : 'application/vnd.ms-excel',
        '.eot'    : 'application/vnd.ms-fontobject',
        '.ppt'    : 'application/vnd.ms-powerpoint',
        '.pot'    : 'application/vnd.ms-powerpoint',
        '.ppa'    : 'application/vnd.ms-powerpoint',
        '.pps'    : 'application/vnd.ms-powerpoint',
        '.pwz'    : 'application/vnd.ms-powerpoint',
        '.odg'    : 'application/vnd.oasis.opendocument.graphics',
        '.odp'    : 'application/vnd.oasis.opendocument.presentation',
        '.ods'    : 'application/vnd.oasis.opendocument.spreadsheet',
        '.odt'    : 'application/vnd.oasis.opendocument.text',
        '.pptx'   : 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
        '.xlsx'   : 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
        '.docx'   : 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
        '.rar'    : 'application/vnd.rar',
        '.wasm'   : 'application/wasm',
        '.7z'     : 'application/x-7z-compressed',
        '.bcpio'  : 'application/x-bcpio',
        '.cpio'   : 'application/x-cpio',
        '.csh'    : 'application/x-csh',
        '.deb'    : 'application/x-debian-package',
        '.dvi'    : 'application/x-dvi',
        '.gtar'   : 'application/x-gtar',
        '.hdf'    : 'application/x-hdf',
        '.h5'     : 'application/x-hdf5',
        '.latex'  : 'application/x-latex',
        '.mif'    : 'application/x-mif',
        '.cdf'    : 'application/x-netcdf',
        '.nc'     : 'application/x-netcdf',
        '.p12'    : 'application/x-pkcs12',
        '.php'    : 'application/x-httpd-php',
        '.pfx'    : 'application/x-pkcs12',
        '.ram'    : 'application/x-pn-realaudio',
        '.pyc'    : 'application/x-python-code',
        '.pyo'    : 'application/x-python-code',
        '.rpm'    : 'application/x-rpm',
        '.sh'     : 'application/x-sh',
        '.shar'   : 'application/x-shar',
        '.swf'    : 'application/x-shockwave-flash',
        '.sv4cpio': 'application/x-sv4cpio',
        '.sv4crc' : 'application/x-sv4crc',
        '.tar'    : 'application/x-tar',
        '.tcl'    : 'application/x-tcl',
        '.tex'    : 'application/x-tex',
        '.texi'   : 'application/x-texinfo',
        '.texinfo': 'application/x-texinfo',
        '.roff'   : 'application/x-troff',
        '.t'      : 'application/x-troff',
        '.tr'     : 'application/x-troff',
        '.man'    : 'application/x-troff-man',
        '.me'     : 'application/x-troff-me',
        '.ms'     : 'application/x-troff-ms',
        '.ustar'  : 'application/x-ustar',
        '.src'    : 'application/x-wais-source',
        '.xsl'    : 'application/xml',
        '.rdf'    : 'application/xml',
        '.wsdl'   : 'application/xml',
        '.xpdl'   : 'application/xml',
        '.yaml'   : 'application/yaml',
        '.yml'    : 'application/yaml',
        '.zip'    : 'application/zip',
        '.3gp'    : 'audio/3gpp',
        '.3gpp'   : 'audio/3gpp',
        '.3g2'    : 'audio/3gpp2',
        '.3gpp2'  : 'audio/3gpp2',
        '.aac'    : 'audio/aac',
        '.adts'   : 'audio/aac',
        '.loas'   : 'audio/aac',
        '.ass'    : 'audio/aac',
        '.au'     : 'audio/basic',
        '.snd'    : 'audio/basic',
        '.flac'   : 'audio/flac',
        '.mka'    : 'audio/matroska',
        '.m4a'    : 'audio/mp4',
        '.mp3'    : 'audio/mpeg',
        '.mp2'    : 'audio/mpeg',
        '.ogg'    : 'audio/ogg',
        '.opus'   : 'audio/opus',
        '.aif'    : 'audio/x-aiff',
        '.aifc'   : 'audio/x-aiff',
        '.aiff'   : 'audio/x-aiff',
        '.ra'     : 'audio/x-pn-realaudio',
        '.wav'    : 'audio/vnd.wave',
        '.otf'    : 'font/otf',
        '.ttf'    : 'font/ttf',
        '.weba'   : 'audio/webm',
        '.woff'   : 'font/woff',
        '.woff2'  : 'font/woff2',
        '.avif'   : 'image/avif',
        '.bmp'    : 'image/bmp',
        '.emf'    : 'image/emf',
        '.fits'   : 'image/fits',
        '.g3'     : 'image/g3fax',
        '.gif'    : 'image/gif',
        '.ief'    : 'image/ief',
        '.jp2'    : 'image/jp2',
        '.jpg'    : 'image/jpeg',
        '.jpe'    : 'image/jpeg',
        '.jpeg'   : 'image/jpeg',
        '.jpm'    : 'image/jpm',
        '.jpx'    : 'image/jpx',
        '.heic'   : 'image/heic',
        '.heif'   : 'image/heif',
        '.png'    : 'image/png',
        '.svg'    : 'image/svg+xml',
        '.t38'    : 'image/t38',
        '.tiff'   : 'image/tiff',
        '.tif'    : 'image/tiff',
        '.tfx'    : 'image/tiff-fx',
        '.ico'    : 'image/vnd.microsoft.icon',
        '.webp'   : 'image/webp',
        '.wmf'    : 'image/wmf',
        '.ras'    : 'image/x-cmu-raster',
        '.pnm'    : 'image/x-portable-anymap',
        '.pbm'    : 'image/x-portable-bitmap',
        '.pgm'    : 'image/x-portable-graymap',
        '.ppm'    : 'image/x-portable-pixmap',
        '.rgb'    : 'image/x-rgb',
        '.xbm'    : 'image/x-xbitmap',
        '.xpm'    : 'image/x-xpixmap',
        '.xwd'    : 'image/x-xwindowdump',
        '.eml'    : 'message/rfc822',
        '.mht'    : 'message/rfc822',
        '.mhtml'  : 'message/rfc822',
        '.nws'    : 'message/rfc822',
        '.gltf'   : 'model/gltf+json',
        '.glb'    : 'model/gltf-binary',
        '.stl'    : 'model/stl',
        '.css'    : 'text/css',
        '.csv'    : 'text/csv',
        '.html'   : 'text/html',
        '.htm'    : 'text/html',
        '.md'     : 'text/markdown',
        '.markdown': 'text/markdown',
        '.n3'     : 'text/n3',
        '.txt'    : 'text/plain',
        '.bat'    : 'text/plain',
        '.c'      : 'text/plain',
        '.h'      : 'text/plain',
        '.ksh'    : 'text/plain',
        '.pl'     : 'text/plain',
        '.srt'    : 'text/plain',
        '.rtx'    : 'text/richtext',
        '.rtf'    : 'text/rtf',
        '.tsv'    : 'text/tab-separated-values',
        '.vtt'    : 'text/vtt',
        '.py'     : 'text/x-python',
        '.rst'    : 'text/x-rst',
        '.etx'    : 'text/x-setext',
        '.sgm'    : 'text/x-sgml',
        '.sgml'   : 'text/x-sgml',
        '.vcf'    : 'text/x-vcard',
        '.xml'    : 'text/xml',
        '.mkv'    : 'video/matroska',
        '.mk3d'   : 'video/matroska-3d',
        '.mp4'    : 'video/mp4',
        '.mpeg'   : 'video/mpeg',
        '.m1v'    : 'video/mpeg',
        '.mpa'    : 'video/mpeg',
        '.mpe'    : 'video/mpeg',
        '.mpg'    : 'video/mpeg',
        '.ogv'    : 'video/ogg',
        '.mov'    : 'video/quicktime',
        '.qt'     : 'video/quicktime',
        '.webm'   : 'video/webm',
        '.avi'    : 'video/vnd.avi',
        '.m4v'    : 'video/x-m4v',
        '.wmv'    : 'video/x-ms-wmv',
        '.movie'  : 'video/x-sgi-movie',
        }


def _common() -> dict[str, str]:
    return {
        '.rtf' : 'application/rtf',
        '.apk' : 'application/vnd.android.package-archive',
        '.midi': 'audio/midi',
        '.mid' : 'audio/midi',
        '.jpg' : 'image/jpg',
        '.pict': 'image/pict',
        '.pct' : 'image/pict',
        '.pic' : 'image/pict',
        '.xul' : 'text/xul',
        }


_state = _State()
