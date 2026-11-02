// Optional build-time OpenSSL protocol flags, following CPython's Modules/_ssl.c.
// The runtime still loads libssl on demand and does not link against it.
#if defined(__has_include)
#  if __has_include(<openssl/ssl.h>)
#    include <openssl/ssl.h>
#  endif
#endif

int imre_ssl_protocol_mask(unsigned long runtime_version) {
#if defined(OPENSSL_VERSION_MAJOR) && OPENSSL_VERSION_MAJOR == 3
    if (runtime_version == OPENSSL_VERSION_NUMBER) {
        int flags = 0;
#if defined(SSL3_VERSION) && !defined(OPENSSL_NO_SSL3)
        flags |= 1 << 0;
#endif
#if defined(TLS1_VERSION) && !defined(OPENSSL_NO_TLS1)
        flags |= 1 << 1;
#endif
#if defined(TLS1_1_VERSION) && !defined(OPENSSL_NO_TLS1_1)
        flags |= 1 << 2;
#endif
#if defined(TLS1_2_VERSION) && !defined(OPENSSL_NO_TLS1_2)
        flags |= 1 << 3;
#endif
#if defined(TLS1_3_VERSION) && !defined(OPENSSL_NO_TLS1_3)
        flags |= 1 << 4;
#endif
        return flags;
    }
#endif
    (void)runtime_version;
    return -1;
}

#include <stddef.h>
#include <stdint.h>

struct imre_ssl_option {
    const char *name;
    uint64_t value;
};

/* Match CPython's exported options, including its adjusted OP_ALL. */
const struct imre_ssl_option *imre_ssl_options(unsigned long runtime_version, size_t *count) {
#if defined(OPENSSL_VERSION_MAJOR) && OPENSSL_VERSION_MAJOR == 3
    if (runtime_version == OPENSSL_VERSION_NUMBER) {
#define OPTION(name) { #name, SSL_##name }
        static const struct imre_ssl_option options[] = {
            {"OP_ALL", SSL_OP_ALL & ~SSL_OP_DONT_INSERT_EMPTY_FRAGMENTS},
            OPTION(OP_NO_SSLv2), OPTION(OP_NO_SSLv3), OPTION(OP_NO_TLSv1),
            OPTION(OP_NO_TLSv1_1), OPTION(OP_NO_TLSv1_2), OPTION(OP_NO_TLSv1_3),
            OPTION(OP_CIPHER_SERVER_PREFERENCE), OPTION(OP_SINGLE_DH_USE),
            OPTION(OP_NO_TICKET), OPTION(OP_LEGACY_SERVER_CONNECT),
#ifdef SSL_OP_SINGLE_ECDH_USE
            OPTION(OP_SINGLE_ECDH_USE),
#endif
#ifdef SSL_OP_NO_COMPRESSION
            OPTION(OP_NO_COMPRESSION),
#endif
#ifdef SSL_OP_ENABLE_MIDDLEBOX_COMPAT
            OPTION(OP_ENABLE_MIDDLEBOX_COMPAT),
#endif
#ifdef SSL_OP_NO_RENEGOTIATION
            OPTION(OP_NO_RENEGOTIATION),
#endif
#ifdef SSL_OP_IGNORE_UNEXPECTED_EOF
            OPTION(OP_IGNORE_UNEXPECTED_EOF),
#endif
#ifdef SSL_OP_ENABLE_KTLS
            OPTION(OP_ENABLE_KTLS),
#endif
        };
#undef OPTION
        *count = sizeof(options) / sizeof(options[0]);
        return options;
    }
#endif
    /* This bit is part of the OpenSSL 3 ABI on every supported provider.
       Leave header-dependent names absent when we cannot verify them. */
    static const struct imre_ssl_option fallback[] = {{"OP_NO_SSLv3", UINT64_C(1) << 25}};
    (void)runtime_version;
    *count = sizeof(fallback) / sizeof(fallback[0]);
    return fallback;
}
