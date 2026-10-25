import os


reader, writer = os.pipe()
os.set_blocking(writer, False)

with open("recovered.bin", "wb") as stream:
    fd = stream.fileno()
    saved = os.dup(fd)
    os.dup2(writer, fd)
    os.close(writer)
    try:
        stream.write(b"\0x\xffy" * (512 * 1024))
        stream.flush()
    except OSError:
        print("blocked")
    finally:
        os.dup2(saved, fd)
        os.close(saved)
    stream.flush()
os.close(reader)
with open("recovered.bin", "rb") as recovered:
    data = recovered.read()
    print(len(data) > 0 and data == b"\0x\xffy" * (len(data) // 4))
