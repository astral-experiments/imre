# Verification

A one-use Ed25519 key generated in memory signed this release. Compare the SHA-256 of its SPKI DER bytes with `7a5048ef4648c26d4bd00be297379d2bc2b2e3b52e063a7afd8f21a85c903a2f` from the coordinating chat. The signature identifies this receipt-specific key and authenticates the listed bytes.

From the publication directory, first use the system OpenSSL and hashing tools.

```sh
openssl pkey -pubin -in signer-ed25519.pem -outform DER | sha256sum
openssl pkeyutl -verify -pubin -inkey signer-ed25519.pem -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum verify-io-writer.py
grep '  tools/verify-io-writer.py$' SHA256SUMS
```

Check that the last two hashes match, then run `python verify-io-writer.py io-buffer-writer-receipts.tar.gz --pin FINGERPRINT_FROM_CHAT` to verify all archive members and signed payload hashes.
