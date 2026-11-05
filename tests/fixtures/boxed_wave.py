import io
import wave

buffer = io.BytesIO()
with wave.open(buffer, "wb") as writer:
    writer.setparams((2, 2, 48000, 0, "NONE", "not compressed"))
    writer.writeframesraw(b"\x01\x00\x02\x00")
    writer.writeframes(b"\x03\x00\x04\x00")

buffer.seek(0)
with wave.open(buffer, "rb") as reader:
    print(reader.getparams())
    print(reader.readframes(1))
    print(reader.tell())
    reader.rewind()
    print(reader.readframes(3))
