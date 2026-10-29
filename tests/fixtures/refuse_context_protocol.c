#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
static _Thread_local unsigned active, count, refused;
static _Thread_local unsigned target;
void *__real_malloc(size_t);
void *__real_realloc(void *, size_t);
void *__real_calloc(size_t,size_t);
uint64_t __real_imre_dyn_op_at(void *,const char *,uint64_t,uint64_t,uint64_t,const uint32_t *);
static int deny(size_t size) {
 if (!active) return 0;
 ++count; unsigned decision=target==count || getenv("CONTEXT_REFUSE_ALL"); active=0;
 dprintf(2,"context_allocation %u %zu %u\n",count,size,decision);
 active=1;
 if(decision) {refused=1;errno=ENOMEM;return 1;}
 return 0;
}
void *__wrap_malloc(size_t n){return deny(n)?NULL:__real_malloc(n);}
void *__wrap_realloc(void *p,size_t n){return deny(n)?NULL:__real_realloc(p,n);}
void *__wrap_calloc(size_t a,size_t b){return deny(a*b)?NULL:__real_calloc(a,b);}
uint64_t __wrap_imre_dyn_op_at(void *ctx,const char *op,uint64_t a,uint64_t b,uint64_t c,const uint32_t *span){
 const char *selected=getenv("CONTEXT_REFUSE_OPCODE");
 if(strcmp(op,selected?selected:"lookup_enter"))return __real_imre_dyn_op_at(ctx,op,a,b,c,span);
 const char *setting=getenv("CONTEXT_REFUSE_NTH");target=setting?strtoul(setting,NULL,10):0;count=0;refused=0;active=1;
 uint64_t result=__real_imre_dyn_op_at(ctx,op,a,b,c,span);active=0;
 dprintf(2,"context_summary %u %u\n",count,refused);return result;
}

uint64_t __real_imre_dyn_op(void *,const char *,uint64_t,uint64_t,uint64_t);
uint64_t __wrap_imre_dyn_op(void *ctx,const char *op,uint64_t a,uint64_t b,uint64_t c){
 const char *selected=getenv("CONTEXT_REFUSE_OPCODE");
 if(strcmp(op,selected?selected:"lookup_enter"))return __real_imre_dyn_op(ctx,op,a,b,c);
 const char *setting=getenv("CONTEXT_REFUSE_NTH");target=setting?strtoul(setting,NULL,10):0;count=0;refused=0;active=1;
 uint64_t result=__real_imre_dyn_op(ctx,op,a,b,c);active=0;dprintf(2,"context_summary %u %u\n",count,refused);return result;
}
