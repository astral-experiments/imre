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

struct imre_ssl_constant {
    const char *name;
    uint64_t value;
};

/* The mandatory alert descriptions are TLS wire codes. The fallback also keeps
   the verification flags supported by the OpenSSL 3 provider without headers. */
#define TLS_ALERTS(X) \
    X(CLOSE_NOTIFY, 0) \
    X(UNEXPECTED_MESSAGE, 10) \
    X(BAD_RECORD_MAC, 20) \
    X(RECORD_OVERFLOW, 22) \
    X(DECOMPRESSION_FAILURE, 30) \
    X(HANDSHAKE_FAILURE, 40) \
    X(BAD_CERTIFICATE, 42) \
    X(UNSUPPORTED_CERTIFICATE, 43) \
    X(CERTIFICATE_REVOKED, 44) \
    X(CERTIFICATE_EXPIRED, 45) \
    X(CERTIFICATE_UNKNOWN, 46) \
    X(ILLEGAL_PARAMETER, 47) \
    X(UNKNOWN_CA, 48) \
    X(ACCESS_DENIED, 49) \
    X(DECODE_ERROR, 50) \
    X(DECRYPT_ERROR, 51) \
    X(PROTOCOL_VERSION, 70) \
    X(INSUFFICIENT_SECURITY, 71) \
    X(INTERNAL_ERROR, 80) \
    X(USER_CANCELLED, 90) \
    X(NO_RENEGOTIATION, 100)

#define VERIFY_FLAGS(X) \
    X(VERIFY_DEFAULT, 0, 0) \
    X(VERIFY_CRL_CHECK_LEAF, X509_V_FLAG_CRL_CHECK, 4) \
    X(VERIFY_CRL_CHECK_CHAIN, (X509_V_FLAG_CRL_CHECK | X509_V_FLAG_CRL_CHECK_ALL), 12) \
    X(VERIFY_X509_STRICT, X509_V_FLAG_X509_STRICT, 32) \
    X(VERIFY_ALLOW_PROXY_CERTS, X509_V_FLAG_ALLOW_PROXY_CERTS, 64) \
    X(VERIFY_X509_TRUSTED_FIRST, X509_V_FLAG_TRUSTED_FIRST, 32768)

/* Match CPython's exported _ssl constants, including its adjusted OP_ALL. */
const struct imre_ssl_constant *imre_ssl_constants(unsigned long runtime_version, size_t *count) {
#if defined(OPENSSL_VERSION_MAJOR) && OPENSSL_VERSION_MAJOR == 3
    if (runtime_version == OPENSSL_VERSION_NUMBER) {
#define OPTION(name) { #name, SSL_##name }
#define ALERT(name, wire) {"ALERT_DESCRIPTION_" #name, SSL_AD_##name},
#define VERIFY(name, value, fallback) {#name, value},
        static const struct imre_ssl_constant constants[] = {
            TLS_ALERTS(ALERT)
            VERIFY_FLAGS(VERIFY)
#ifdef X509_V_FLAG_PARTIAL_CHAIN
            {"VERIFY_X509_PARTIAL_CHAIN", X509_V_FLAG_PARTIAL_CHAIN},
#endif
#ifdef SSL_AD_UNSUPPORTED_EXTENSION
            {"ALERT_DESCRIPTION_UNSUPPORTED_EXTENSION", SSL_AD_UNSUPPORTED_EXTENSION},
#endif
#ifdef SSL_AD_CERTIFICATE_UNOBTAINABLE
            {"ALERT_DESCRIPTION_CERTIFICATE_UNOBTAINABLE", SSL_AD_CERTIFICATE_UNOBTAINABLE},
#endif
#ifdef SSL_AD_UNRECOGNIZED_NAME
            {"ALERT_DESCRIPTION_UNRECOGNIZED_NAME", SSL_AD_UNRECOGNIZED_NAME},
#endif
#ifdef SSL_AD_BAD_CERTIFICATE_STATUS_RESPONSE
            {"ALERT_DESCRIPTION_BAD_CERTIFICATE_STATUS_RESPONSE", SSL_AD_BAD_CERTIFICATE_STATUS_RESPONSE},
#endif
#ifdef SSL_AD_BAD_CERTIFICATE_HASH_VALUE
            {"ALERT_DESCRIPTION_BAD_CERTIFICATE_HASH_VALUE", SSL_AD_BAD_CERTIFICATE_HASH_VALUE},
#endif
#ifdef SSL_AD_UNKNOWN_PSK_IDENTITY
            {"ALERT_DESCRIPTION_UNKNOWN_PSK_IDENTITY", SSL_AD_UNKNOWN_PSK_IDENTITY},
#endif
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
#undef ALERT
#undef VERIFY
        *count = sizeof(constants) / sizeof(constants[0]);
        return constants;
    }
#endif
    /* Keep ABI-stable values when there are no matching headers. */
#define ALERT(name, wire) {"ALERT_DESCRIPTION_" #name, wire},
#define VERIFY(name, value, fallback) {#name, fallback},
    static const struct imre_ssl_constant fallback[] = {
        TLS_ALERTS(ALERT)
        VERIFY_FLAGS(VERIFY)
        {"VERIFY_X509_PARTIAL_CHAIN", UINT64_C(1) << 19},
        {"OP_NO_SSLv3", UINT64_C(1) << 25},
    };
#undef ALERT
#undef VERIFY
    (void)runtime_version;
    *count = sizeof(fallback) / sizeof(fallback[0]);
    return fallback;
}
