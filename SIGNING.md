# Verification

A one-use Ed25519 key generated in memory signed this release. Compare the SHA-256 of its SPKI DER bytes with `dc52e20539845629ca06cde5fde050888deb069f8b6c00ebc48369d96c0565a6` from the coordinating chat. The signature identifies this receipt-specific key and authenticates the listed bytes.

From the publication directory, first use the system OpenSSL and hashing tools.

```sh
openssl pkey -pubin -in signer-ed25519.pem -outform DER | sha256sum
openssl pkeyutl -verify -pubin -inkey signer-ed25519.pem -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum verify-cbwc.py
grep '  tools/verify-cbwc.py$' SHA256SUMS
```

Check that the last two hashes match, then run `python verify-cbwc.py cbwc-receipts.tar.gz --pin FINGERPRINT_FROM_CHAT` to verify all archive members and signed payload hashes.
