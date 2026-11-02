# Verification

A one-use Ed25519 key generated in memory signed this release. Compare the SHA-256 of its SPKI DER bytes with `6a012d499087c2b1f7c54707ca925c8e2e5d07d2d5025d94289254021db59ecc` from the coordinating chat. The signature identifies this receipt-specific key and authenticates the listed bytes.

From the publication directory, first use the system OpenSSL and hashing tools.

```sh
openssl pkey -pubin -in signer-ed25519.pem -outform DER | sha256sum
openssl pkeyutl -verify -pubin -inkey signer-ed25519.pem -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum verify-io-binding.py
grep '  tools/verify-io-binding.py$' SHA256SUMS
```

Check that the last two hashes match, then run `python verify-io-binding.py io-buffer-binding-receipts.tar.gz --pin FINGERPRINT_FROM_CHAT` to verify all archive members and signed payload hashes.
