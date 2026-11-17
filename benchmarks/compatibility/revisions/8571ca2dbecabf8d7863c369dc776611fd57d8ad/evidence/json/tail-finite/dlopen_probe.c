#include <dlfcn.h>
int main(void) { return dlopen("/not-present/libpython3.14.so", RTLD_LAZY) ? 1 : 0; }
