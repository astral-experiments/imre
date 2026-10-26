#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <stdio.h>
#include <string.h>
int main(void) {
    struct in_addr addr;
    inet_pton(AF_INET, "127.0.0.1", &addr);
    struct hostent *h = gethostbyaddr(&addr, sizeof(addr), AF_INET);
    if (!h) { printf("C host=NULL h_errno=%d errno=%d\n", h_errno, errno); return 0; }
    printf("C name=%s family=%d len=%d\n", h->h_name, h->h_addrtype, h->h_length);
    for (char **s=h->h_aliases; s && *s; ++s) printf("C alias=%s\n", *s);
    return 0;
}
