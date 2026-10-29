#include <errno.h>
#include <stdbool.h>
#include <stdlib.h>
#include <unistd.h>
static void *buffers[128];
static bool refused;
extern void *__real_malloc(size_t);
extern void *__real_realloc(void*,size_t);
extern void *__real_calloc(size_t,size_t);
extern void __real_free(void*);
static int slot(void *p) {
    if(p) for(int i=0;i<128;i++) if(buffers[i]==p) return i;
    return -1;
}
static void forget(void *p) { int i=slot(p);if(i>=0)buffers[i]=NULL; }
static void remember(void *p,size_t n) {
    if(p && n==4096) for(int i=0;i<128;i++) if(!buffers[i]) {buffers[i]=p;return;}
}
void *__wrap_malloc(size_t n) {void *p=__real_malloc(n);remember(p,n);return p;}
void *__wrap_calloc(size_t n,size_t s) {
    void *p=__real_calloc(n,s);
    if(s && n<=((size_t)-1)/s)remember(p,n*s);
    return p;
}
void *__wrap_realloc(void *p,size_t n) {
    int previous=slot(p);
    const char *enabled=getenv("IMRE_TEST_IO_COMPACTION_ALLOCATION");
    if(!refused && previous>=0 && n==32 && enabled && enabled[0]=='1') {
        refused=true;
        static const char marker[]="refused\n";
        write(2,marker,sizeof(marker)-1);
        errno=ENOMEM;
        return NULL;
    }
    void *result=__real_realloc(p,n);
    if((result || !n) && previous>=0)buffers[previous]=NULL;
    remember(result,n);
    return result;
}
void __wrap_free(void *p) {forget(p);__real_free(p);}
