from email import message_from_binary_file, message_from_bytes, message_from_string
from email.errors import MissingHeaderBodySeparatorDefect
from email.feedparser import FeedParser
from email.message import Message
from io import BytesIO


def current():
    text = "X-Trace: one\r\nx-trace: two\r\n\tcontinued\r\n\r\nbody"
    msg = message_from_string(text)
    assert type(msg) is Message
    assert msg.get_all("X-Trace") == ["one", "two\r\n\tcontinued"]
    print(msg.items(), msg.get_payload())
    msg = message_from_bytes(b"X-Trace: wire\r\n\r\nbody\xff")
    assert msg.items() == [("X-Trace", "wire")]
    assert msg.get_payload(decode=True) == b"body\xff"
    msg["Content-Type"] = "text/plain; charset=UTF-8"
    assert msg.get_content_charset() == "utf-8"
    assert msg.get_charset() is None
    payload_charset = object()
    msg._charset = payload_charset
    assert msg.get_charset() is payload_charset
    with BytesIO(b"X-Trace: wire\r\nbroken\r\nbody\xff") as fp:
        msg = message_from_binary_file(fp)
        assert fp.read() == b""
    assert msg["X-Trace"] == "wire"
    assert msg.get_payload(decode=True) == b"broken\nbody\xff"
    assert [type(defect) for defect in msg.defects] == [MissingHeaderBodySeparatorDefect]
    parser = FeedParser()
    parser.feed("broken head")
    parser.feed("er\r\nbody")
    msg = parser.close()
    print([type(defect).__name__ for defect in msg.defects], msg.get_payload())

    from email import policy
    from email.parser import BytesParser

    wire = (
        b"Subject: =?utf-8?q?caf=C3=A9?=\r\n"
        b'Content-Type: multipart/mixed; boundary="parts"\r\n\r\n'
        b"--parts\r\nContent-Type: text/plain\r\n\r\nhello\r\n"
        b"--parts\r\nContent-Type: application/octet-stream\r\n"
        b"Content-Disposition: attachment; filename*=utf-8''caf%C3%A9.bin\r\n"
        b"Content-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--parts--\r\n"
    )
    message = BytesParser(policy=policy.default).parsebytes(wire)
    assert str(message["Subject"]) == "café"
    body, attachment = message.iter_parts()
    assert body.get_content() == "hello"
    assert attachment.get_filename() == "café.bin"
    assert attachment.get_payload(decode=True) == b"\x00\xff"
