#include <dlfcn.h>
int main(void) { return dlopen("libpython-test-probe.so", RTLD_NOW) ? 2 : 0; }
