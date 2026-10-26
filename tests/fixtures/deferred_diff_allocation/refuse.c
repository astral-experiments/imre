#include <stdint.h>
#include <unistd.h>
#include <errno.h>
#include <stdlib.h>
#include <stdio.h>
/* The first string print follows construction of both snapshots and both cursors.
 * Refuse an actual allocation in the observation, then leave recovery unarmed. */
extern void __real_imre_print_str(const void *);
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t,size_t);
extern void *__real_realloc(void *,size_t);
static unsigned calls, prints, target;
static int armed;
void __wrap_imre_print_str(const void *s) {
    armed=0; __real_imre_print_str(s);
    if (++prints==1) {
        const char *n=getenv("IMRE_DIFF_FAIL_N");
        target=n ? (unsigned)strtoul(n, NULL, 10) : 0;
        armed=1;
    }
}
static int refuse(size_t bytes) {
    if (!armed || ++calls!=target) return 0;
    armed=0; char message[96];
    int n=snprintf(message,sizeof(message),"refused %u %zu\n",calls,bytes);
    (void)write(2,message,(size_t)n); errno=ENOMEM; return 1;
}
void *__wrap_malloc(size_t n) {return refuse(n) ? NULL : __real_malloc(n);}
void *__wrap_calloc(size_t n,size_t m) {return refuse(n*m) ? NULL : __real_calloc(n,m);}
void *__wrap_realloc(void *p,size_t n) {return refuse(n) ? NULL : __real_realloc(p,n);}
__attribute__((destructor)) static void report(void) {
    char line[80];int n=snprintf(line,sizeof(line),"observed %u\n",calls);(void)write(2,line,(size_t)n);
}
