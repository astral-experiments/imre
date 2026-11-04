import ssl
import sys

certdata = sys.argv[1]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.load_verify_locations(cafile=certdata + "/keycert2.pem")
assert ctx.get_ca_certs() == []
with open(certdata + "/ssl_cert.pem", encoding="ascii") as f:
    pem = f.read()
cafile = certdata + "/selfsigned_pythontestdotnet.pem"
ctx.load_verify_locations(cafile=cafile, capath=certdata, cadata=pem)
stats = {"x509": 3, "crl": 0, "x509_ca": 1}
assert ctx.cert_store_stats() == stats
der = ssl.PEM_cert_to_DER_cert(pem)
ctx.load_verify_locations(cadata=der)
assert ctx.cert_store_stats() == stats
with open(cafile, encoding="ascii") as f:
    ca_der = ssl.PEM_cert_to_DER_cert(f.read())
assert ctx.get_ca_certs(binary_form=True) == [ca_der]
[authority] = ctx.get_ca_certs()
assert authority["issuer"] == authority["subject"]
assert authority["subject"][-1] == (("commonName", "self-signed.pythontest.net"),)
authority.clear()
assert ctx.get_ca_certs()[0]["version"] == 3

partial = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
try:
    partial.load_verify_locations(cafile=certdata + "/absent.pem", cadata=der)
except FileNotFoundError as exc:
    assert exc.errno == 2
else:
    raise AssertionError("accepted a missing cafile")
assert partial.cert_store_stats()["x509"] == 1
try:
    partial.load_verify_locations(cadata=b"not a certificate")
except ssl.SSLError:
    pass
else:
    raise AssertionError("accepted invalid DER")
assert partial.cert_store_stats()["x509"] == 1


class LoadAuthority:
    def __bool__(self):
        partial.load_verify_locations(cafile=cafile)
        return True


assert partial.get_ca_certs(LoadAuthority()) == [ca_der]

factory = ssl._create_unverified_context
assert ssl._create_stdlib_context is factory
unverified = factory()
assert unverified.protocol == ssl.PROTOCOL_TLS_CLIENT
assert not unverified.check_hostname and unverified.verify_mode == ssl.CERT_NONE
verified = factory(cert_reqs=ssl.CERT_REQUIRED, check_hostname=True, cafile=cafile)
assert verified.check_hostname and verified.verify_mode == ssl.CERT_REQUIRED
assert verified.get_ca_certs(binary_form=True) == [ca_der]
assert factory(purpose=ssl.Purpose.CLIENT_AUTH).protocol == ssl.PROTOCOL_TLS_SERVER
try:
    factory(cert_reqs=ssl.CERT_NONE, check_hostname=True)
except ValueError:
    pass
else:
    raise AssertionError("accepted CERT_NONE with hostname checking")
