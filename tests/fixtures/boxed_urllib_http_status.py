def current():
    import io
    from email.message import Message
    from urllib import request
    from urllib.error import HTTPError
    from urllib.response import addinfourl

    class Replies(request.BaseHandler):
        handler_order = 10

        def http_open(self, req):
            headers = Message()
            if req.selector == "/start":
                headers["Location"] = "/destination"
                code = 302
            elif req.selector == "/forbidden":
                headers["Location"] = "file:///private"
                code = 302
            else:
                code = 401
            response = addinfourl(io.BytesIO(b"denied"), headers, req.full_url, code)
            response.msg = "fixture status"
            return response

    opener = request.build_opener(Replies())
    for path, code, url in [("/start", 401, "http://127.0.0.1/destination"),
                            ("/forbidden", 302, "file:///private")]:
        try:
            opener.open("http://127.0.0.1" + path)
        except HTTPError as error:
            assert error.getcode() == code
            assert error.geturl() == url
            assert error.read() == b"denied"
            error.close()
            assert error.closed
        else:
            raise AssertionError("the opener returned an error response")
