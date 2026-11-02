# Verification

A one-use Ed25519 key generated in memory signed this release. Compare the SHA-256 of its SPKI DER bytes with `10156c72664e3c6b87081d9a2e118054f3b9b49ceef0b33b9bcbf82173fee23a` from the coordinating chat. The signature identifies this receipt-specific key and authenticates the listed bytes.

From the publication directory, first use the system OpenSSL and hashing tools.

```sh
openssl pkey -pubin -in signer-ed25519.pem -outform DER | sha256sum
openssl pkeyutl -verify -pubin -inkey signer-ed25519.pem -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum verify-cbwr.py
grep '  tools/verify-cbwr.py$' SHA256SUMS
```

Check that the last two hashes match, then run `python verify-cbwr.py cbwr-receipts.tar.gz --pin FINGERPRINT_FROM_CHAT` to verify all archive members and signed payload hashes.
