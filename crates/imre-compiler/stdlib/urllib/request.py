# URL request, local path and environment proxy helpers from CPython 3.14.5
# Lib/urllib/request.py. See ../docstrings/LICENSE.
import base64
import bisect
import io
import os
import re
import socket
import sys
import tempfile

from email.message import Message
from urllib.error import ContentTooShortError, URLError
from urllib.response import addinfourl
from urllib.parse import (
    _splithost, _splitport, _splittag, _splittype, quote, unquote, unquote_to_bytes, unwrap, urlparse, urlsplit,
)


__all__ = [
    "Request", "OpenerDirector", "BaseHandler", "DataHandler", "FileHandler", "UnknownHandler",
    "urlopen", "install_opener", "build_opener", "urlretrieve", "urlcleanup",
    "pathname2url", "url2pathname",
]


__version__ = '%d.%d' % sys.version_info[:2]

_opener = None
def urlopen(url, data=None, timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
            *, context=None):
    """Open a URL or Request through the installed or default opener."""
    global _opener
    if context:
        # The context selects a fresh opener. Only an HTTPS handler consumes it.
        opener = build_opener()
    elif _opener is None:
        _opener = opener = build_opener()
    else:
        opener = _opener
    return opener.open(url, data, timeout)

def install_opener(opener):
    global _opener
    _opener = opener


_url_tempfiles = []

def urlretrieve(url, filename=None, reporthook=None, data=None):
    """
    Retrieve a URL into a temporary location on disk.

    Requires a URL argument. If a filename is passed, it is used as
    the temporary file location. The reporthook argument should be
    a callable that accepts a block number, a read size, and the
    total file size of the URL target. The data argument should be
    valid URL encoded data.

    If a filename is passed and the URL points to a local resource,
    the result is a copy from local file to new file.

    Returns a tuple containing the path to the newly created
    data file as well as the resulting HTTPMessage object.
    """
    url_type, path = _splittype(url)

    fp = urlopen(url, data)
    try:
        headers = fp.info()

        # Just return the local path and the "headers" for file://
        # URLs. No sense in performing a copy unless requested.
        if url_type == "file" and not filename:
            return os.path.normpath(path), headers

        # Handle temporary file setup.
        if filename:
            tfp = open(filename, 'wb')
        else:
            tfp = tempfile.NamedTemporaryFile(delete=False)
            filename = tfp.name
            _url_tempfiles.append(filename)

        with tfp:
            result = filename, headers
            bs = 1024*8
            size = -1
            read = 0
            blocknum = 0
            if "content-length" in headers:
                size = int(headers["Content-Length"])

            if reporthook:
                reporthook(blocknum, bs, size)

            while block := fp.read(bs):
                read += len(block)
                tfp.write(block)
                blocknum += 1
                if reporthook:
                    reporthook(blocknum, bs, size)
    finally:
        fp.close()

    if size >= 0 and read < size:
        raise ContentTooShortError(
            "retrieval incomplete: got only %i out of %i bytes"
            % (read, size), result)

    return result

def urlcleanup():
    """Clean up temporary files from urlretrieve calls."""
    for temp_file in _url_tempfiles:
        try:
            os.unlink(temp_file)
        except OSError:
            pass

    del _url_tempfiles[:]
    global _opener
    if _opener:
        _opener = None

_cut_port_re = re.compile(r":\d+$", re.ASCII)
def request_host(request):
    """Return request-host, as defined by RFC 2965.

    Variation from RFC: returned value is lowercased, for convenient
    comparison.

    """
    url = request.full_url
    host = urlparse(url)[1]
    if host == "":
        host = request.get_header("Host", "")

    # remove port, if present
    host = _cut_port_re.sub("", host, 1)
    return host.lower()

class Request:

    def __init__(self, url, data=None, headers={},
                 origin_req_host=None, unverifiable=False,
                 method=None):
        self.full_url = url
        self.headers = {}
        self.unredirected_hdrs = {}
        self._data = None
        self.data = data
        self._tunnel_host = None
        for key, value in headers.items():
            self.add_header(key, value)
        if origin_req_host is None:
            origin_req_host = request_host(self)
        self.origin_req_host = origin_req_host
        self.unverifiable = unverifiable
        if method:
            self.method = method

    @property
    def full_url(self):
        if self.fragment:
            return '{}#{}'.format(self._full_url, self.fragment)
        return self._full_url

    @full_url.setter
    def full_url(self, url):
        # unwrap('<URL:type://host/path>') --> 'type://host/path'
        self._full_url = unwrap(url)
        self._full_url, self.fragment = _splittag(self._full_url)
        self._parse()

    @full_url.deleter
    def full_url(self):
        self._full_url = None
        self.fragment = None
        self.selector = ''

    @property
    def data(self):
        return self._data

    @data.setter
    def data(self, data):
        if data != self._data:
            self._data = data
            # issue 16464
            # if we change data we need to remove content-length header
            # (cause it's most probably calculated for previous value)
            if self.has_header("Content-length"):
                self.remove_header("Content-length")

    @data.deleter
    def data(self):
        self.data = None

    def _parse(self):
        self.type, rest = _splittype(self._full_url)
        if self.type is None:
            raise ValueError("unknown url type: %r" % self.full_url)
        self.host, self.selector = _splithost(rest)
        if self.host:
            self.host = unquote(self.host)

    def get_method(self):
        """Return a string indicating the HTTP request method."""
        default_method = "POST" if self.data is not None else "GET"
        return getattr(self, 'method', default_method)

    def get_full_url(self):
        return self.full_url

    def set_proxy(self, host, type):
        if self.type == 'https' and not self._tunnel_host:
            self._tunnel_host = self.host
        else:
            self.type= type
            self.selector = self.full_url
        self.host = host

    def has_proxy(self):
        return self.selector == self.full_url

    def add_header(self, key, val):
        # useful for something like authentication
        self.headers[key.capitalize()] = val

    def add_unredirected_header(self, key, val):
        # will not be added to a redirected request
        self.unredirected_hdrs[key.capitalize()] = val

    def has_header(self, header_name):
        return (header_name in self.headers or
                header_name in self.unredirected_hdrs)

    def get_header(self, header_name, default=None):
        return self.headers.get(
            header_name,
            self.unredirected_hdrs.get(header_name, default))

    def remove_header(self, header_name):
        self.headers.pop(header_name, None)
        self.unredirected_hdrs.pop(header_name, None)

    def header_items(self):
        hdrs = {**self.unredirected_hdrs, **self.headers}
        return list(hdrs.items())


class OpenerDirector:
    def __init__(self):
        client_version = "Python-urllib/%s" % __version__
        self.addheaders = [('User-agent', client_version)]
        # self.handlers is retained only for backward compatibility
        self.handlers = []
        # manage the individual handlers
        self.handle_open = {}
        self.handle_error = {}
        self.process_response = {}
        self.process_request = {}

    def add_handler(self, handler):
        if not hasattr(handler, "add_parent"):
            raise TypeError("expected BaseHandler instance, got %r" %
                            type(handler))

        added = False
        for meth in dir(handler):
            if meth in ["redirect_request", "do_open", "proxy_open"]:
                # oops, coincidental match
                continue

            i = meth.find("_")
            protocol = meth[:i]
            condition = meth[i+1:]

            if condition.startswith("error"):
                j = condition.find("_") + i + 1
                kind = meth[j+1:]
                try:
                    kind = int(kind)
                except ValueError:
                    pass
                lookup = self.handle_error.get(protocol, {})
                self.handle_error[protocol] = lookup
            elif condition == "open":
                kind = protocol
                lookup = self.handle_open
            elif condition == "response":
                kind = protocol
                lookup = self.process_response
            elif condition == "request":
                kind = protocol
                lookup = self.process_request
            else:
                continue

            handlers = lookup.setdefault(kind, [])
            if handlers:
                bisect.insort(handlers, handler)
            else:
                handlers.append(handler)
            added = True

        if added:
            bisect.insort(self.handlers, handler)
            handler.add_parent(self)

    def close(self):
        # Only exists for backwards compatibility.
        pass

    def _call_chain(self, chain, kind, meth_name, *args):
        # Handlers raise an exception if no one else should try to handle
        # the request, or return None if they can't but another handler
        # could.  Otherwise, they return the response.
        handlers = chain.get(kind, ())
        for handler in handlers:
            func = getattr(handler, meth_name)
            result = func(*args)
            if result is not None:
                return result

    def open(self, fullurl, data=None, timeout=socket._GLOBAL_DEFAULT_TIMEOUT):
        # accept a URL or a Request object
        if isinstance(fullurl, str):
            req = Request(fullurl, data)
        else:
            req = fullurl
            if data is not None:
                req.data = data

        req.timeout = timeout
        protocol = req.type

        # pre-process request
        meth_name = protocol+"_request"
        for processor in self.process_request.get(protocol, []):
            meth = getattr(processor, meth_name)
            req = meth(req)

        sys.audit('urllib.Request', req.full_url, req.data, req.headers, req.get_method())
        response = self._open(req, data)

        # post-process response
        meth_name = protocol+"_response"
        for processor in self.process_response.get(protocol, []):
            meth = getattr(processor, meth_name)
            response = meth(req, response)

        return response

    def _open(self, req, data=None):
        result = self._call_chain(self.handle_open, 'default',
                                  'default_open', req)
        if result:
            return result

        protocol = req.type
        result = self._call_chain(self.handle_open, protocol, protocol +
                                  '_open', req)
        if result:
            return result

        return self._call_chain(self.handle_open, 'unknown',
                                'unknown_open', req)

    def error(self, proto, *args):
        if proto in ('http', 'https'):
            # XXX http[s] protocols are special-cased
            dict = self.handle_error['http'] # https is not different than http
            proto = args[2]  # YUCK!
            meth_name = 'http_error_%s' % proto
            http_err = 1
            orig_args = args
        else:
            dict = self.handle_error
            meth_name = proto + '_error'
            http_err = 0
        args = (dict, proto, meth_name) + args
        result = self._call_chain(*args)
        if result:
            return result

        if http_err:
            args = (dict, 'default', 'http_error_default') + orig_args
            return self._call_chain(*args)


def build_opener(*handlers):
    """Create an opener object from a list of handlers.

    The default opener supports local files. Other protocols can be
    supplied by handlers.

    If any of the handlers passed as arguments are subclasses of the
    default handlers, the default handlers will not be used.
    """
    opener = OpenerDirector()
    default_classes = [UnknownHandler, FileHandler, DataHandler]
    skip = set()
    for klass in default_classes:
        for check in handlers:
            if isinstance(check, type):
                if issubclass(check, klass):
                    skip.add(klass)
            elif isinstance(check, klass):
                skip.add(klass)
    for klass in skip:
        default_classes.remove(klass)

    for klass in default_classes:
        handler = _DefaultUnknownHandler() if klass is UnknownHandler else klass()
        opener.add_handler(handler)

    for h in handlers:
        if isinstance(h, type):
            h = h()
        opener.add_handler(h)
    return opener


class BaseHandler:
    handler_order = 500

    def add_parent(self, parent):
        self.parent = parent

    def close(self):
        # Only exists for backwards compatibility
        pass

    def __lt__(self, other):
        if not hasattr(other, "handler_order"):
            # Try to preserve the old behavior of having custom classes
            # inserted after default ones (works only for custom user
            # classes which are not aware of handler_order).
            return True
        return self.handler_order < other.handler_order


class UnknownHandler(BaseHandler):
    def unknown_open(self, req):
        raise URLError('unknown url type: %s' % req.type)


class _DefaultUnknownHandler(UnknownHandler):
    def unknown_open(self, req):
        if req.type in ("http", "https", "ftp"):
            raise NotImplementedError("urllib.request has no default %s handler" % req.type)
        return super().unknown_open(req)


class DataHandler(BaseHandler):
    def data_open(self, req):
        # data URLs as specified in RFC 2397.
        #
        # ignores POSTed data
        #
        # syntax:
        # dataurl   := "data:" [ mediatype ] [ ";base64" ] "," data
        # mediatype := [ type "/" subtype ] *( ";" parameter )
        # data      := *urlchar
        # parameter := attribute "=" value
        url = req.full_url

        scheme, data = url.split(":",1)
        mediatype, data = data.split(",",1)

        # Disallow control characters within mediatype.
        if re.search(r"[\x00-\x1F\x7F]", mediatype):
            raise ValueError(
                "Control characters not allowed in data: mediatype")

        # even base64 encoded data URLs might be quoted so unquote in any case:
        data = unquote_to_bytes(data)
        if mediatype.endswith(";base64"):
            try:
                decode = base64.decodebytes
            except AttributeError:
                # decodebytes passes this already-decoded bytes object to binascii.
                decode = base64.binascii.a2b_base64
            data = decode(data)
            mediatype = mediatype[:-7]

        if not mediatype:
            mediatype = "text/plain;charset=US-ASCII"

        headers = Message()
        # Parsing the generated header consumes spaces after the colon.
        headers["Content-type"] = mediatype.lstrip(" ")
        headers["Content-length"] = str(len(data))
        return addinfourl(io.BytesIO(data), headers, url)


class FileHandler(BaseHandler):
    # names for the localhost
    names = None
    def get_names(self):
        if FileHandler.names is None:
            try:
                FileHandler.names = tuple(
                    _host_addresses('localhost') + _host_addresses(socket.gethostname()))
            except socket.gaierror:
                FileHandler.names = (socket.gethostbyname('localhost'),)
        return FileHandler.names

    # not entirely sure what the rules are here
    def open_local_file(self, req):
        import email.utils
        import mimetypes
        localfile = url2pathname(req.full_url, require_scheme=True, resolve_host=True)
        try:
            stats = os.stat(localfile)
            size = stats.st_size
            modified = email.utils.formatdate(stats.st_mtime, usegmt=True)
            mtype = mimetypes.guess_file_type(localfile)[0]
            headers = Message()
            headers['Content-type'] = mtype or 'text/plain'
            headers['Content-length'] = str(size)
            headers['Last-modified'] = modified
            origurl = pathname2url(localfile, add_scheme=True)
            return addinfourl(open(localfile, 'rb'), headers, origurl)
        except OSError as exp:
            raise URLError(exp, exp.filename)

    file_open = open_local_file


def _is_local_authority(authority, resolve):
    # Compare hostnames
    if not authority or authority == 'localhost':
        return True
    try:
        hostname = socket.gethostname()
    except (socket.gaierror, AttributeError):
        pass
    else:
        if authority == hostname:
            return True
    # Compare IP addresses
    if not resolve:
        return False
    try:
        address = socket.gethostbyname(authority)
    except (socket.gaierror, AttributeError, UnicodeEncodeError):
        return False
    return address in FileHandler().get_names()


def _host_addresses(host):
    # getaddrinfo can supply IPv4 addresses when the socket provider lacks aliases.
    if hasattr(socket, 'gethostbyname_ex'):
        return socket.gethostbyname_ex(host)[2]
    return [entry[4][0] for entry in
            socket.getaddrinfo(host, 0, socket.AF_INET, socket.SOCK_STREAM)]


def url2pathname(url, *, require_scheme=False, resolve_host=False):
    """Convert the given file URL to a local file system path.

    The 'file:' scheme prefix must be omitted unless *require_scheme*
    is set to true.

    The URL authority may be resolved with gethostbyname() if
    *resolve_host* is set to true.
    """
    if not require_scheme:
        url = 'file:' + url
    scheme, authority, url = urlsplit(url)[:3]  # Discard query and fragment.
    if scheme != 'file':
        raise URLError("URL is missing a 'file:' scheme")
    if os.name == 'nt':
        if authority[1:2] == ':':
            # e.g. file://c:/file.txt
            url = authority + url
        elif not _is_local_authority(authority, resolve_host):
            # e.g. file://server/share/file.txt
            url = '//' + authority + url
        elif url[:3] == '///':
            # e.g. file://///server/share/file.txt
            url = url[1:]
        else:
            if url[:1] == '/' and url[2:3] in (':', '|'):
                # Skip past extra slash before DOS drive in URL path.
                url = url[1:]
            if url[1:2] == '|':
                # Older URLs use a pipe after a drive letter
                url = url[:1] + ':' + url[2:]
        url = url.replace('/', '\\')
    elif not _is_local_authority(authority, resolve_host):
        raise URLError("file:// scheme is supported only on localhost")
    encoding = sys.getfilesystemencoding()
    errors = sys.getfilesystemencodeerrors()
    return unquote(url, encoding=encoding, errors=errors)


def pathname2url(pathname, *, add_scheme=False):
    """Convert the given local file system path to a file URL.

    The 'file:' scheme prefix is omitted unless *add_scheme*
    is set to true.
    """
    if os.name == 'nt':
        pathname = pathname.replace('\\', '/')
    encoding = sys.getfilesystemencoding()
    errors = sys.getfilesystemencodeerrors()
    scheme = 'file:' if add_scheme else ''
    drive, root, tail = os.path.splitroot(pathname)
    if drive:
        # First, clean up some special forms. We are going to sacrifice the
        # additional information anyway
        if drive[:4] == '//?/':
            drive = drive[4:]
            if drive[:4].upper() == 'UNC/':
                drive = '//' + drive[4:]
        if drive[1:] == ':':
            # DOS drive specified. Add three slashes to the start, producing
            # an authority section with a zero-length authority, and a path
            # section starting with a single slash.
            drive = '///' + drive
        drive = quote(drive, encoding=encoding, errors=errors, safe='/:')
    elif root:
        # Add explicitly empty authority to absolute path. If the path
        # starts with exactly one slash then this change is mostly
        # cosmetic, but if it begins with two or more slashes then this
        # avoids interpreting the path as a URL authority.
        root = '//' + root
    tail = quote(tail, encoding=encoding, errors=errors)
    return scheme + drive + root + tail


# Utility functions

_localhost = None
def localhost():
    """Return the IP address of the magic hostname 'localhost'."""
    global _localhost
    if _localhost is None:
        _localhost = socket.gethostbyname('localhost')
    return _localhost

_thishost = None
def thishost():
    """Return the IP addresses of the current host."""
    global _thishost
    if _thishost is None:
        try:
            _thishost = tuple(_host_addresses(socket.gethostname()))
        except socket.gaierror:
            _thishost = tuple(_host_addresses('localhost'))
    return _thishost


def getproxies_environment():
    """Return a dictionary of scheme -> proxy server URL mappings.

    Scan the environment for variables named <scheme>_proxy;
    this seems to be the standard convention.
    """
    # in order to prefer lowercase variables, process environment in
    # two passes: first matches any, second pass matches lowercase only

    # select only environment variables which end in (after making lowercase) _proxy
    proxies = {}
    environment = []
    for name in os.environ:
        # fast screen underscore position before more expensive case-folding
        if len(name) > 5 and name[-6] == "_" and name[-5:].lower() == "proxy":
            value = os.environ[name]
            proxy_name = name[:-6].lower()
            environment.append((name, value, proxy_name))
            if value:
                proxies[proxy_name] = value
    # CVE-2016-1000110 - If we are running as CGI script, forget HTTP_PROXY
    # (non-all-lowercase) as it may be set from the web server by a "Proxy:"
    # header from the client
    # If "proxy" is lowercase, it will still be used thanks to the next block
    if 'REQUEST_METHOD' in os.environ:
        proxies.pop('http', None)
    for name, value, proxy_name in environment:
        # not case-folded, checking here for lower-case env vars only
        if name[-6:] == '_proxy':
            if value:
                proxies[proxy_name] = value
            else:
                proxies.pop(proxy_name, None)
    return proxies

def proxy_bypass_environment(host, proxies=None):
    """Test if proxies should not be used for a particular host.

    Checks the proxy dict for the value of no_proxy, which should
    be a list of comma separated DNS suffixes, or '*' for all hosts.

    """
    if proxies is None:
        proxies = getproxies_environment()
    # don't bypass, if no_proxy isn't specified
    try:
        no_proxy = proxies['no']
    except KeyError:
        return False
    # '*' is special case for always bypass
    if no_proxy == '*':
        return True
    host = host.lower()
    # strip port off host
    hostonly, port = _splitport(host)
    # check if the host ends with any of the DNS suffixes
    for name in no_proxy.split(','):
        name = name.strip()
        if name:
            name = name.lstrip('.')  # ignore leading dots
            name = name.lower()
            if hostonly == name or host == name:
                return True
            name = '.' + name
            if hostonly.endswith(name) or host.endswith(name):
                return True
    # otherwise, don't bypass
    return False
