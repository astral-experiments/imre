from email import message_from_string
from email.feedparser import FeedParser
from email.message import Message


def current():
    text = "X-Trace: one\r\nx-trace: two\r\n\tcontinued\r\n\r\nbody"
    msg = message_from_string(text)
    assert type(msg) is Message
    assert msg.get_all("X-Trace") == ["one", "two\r\n\tcontinued"]
    print(msg.items(), msg.get_payload())
    parser = FeedParser()
    parser.feed("broken head")
    parser.feed("er\r\nbody")
    msg = parser.close()
    print([type(defect).__name__ for defect in msg.defects], msg.get_payload())
