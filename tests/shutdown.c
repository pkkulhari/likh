/* Exercise production teardown with real dlopen/dlclose and a simulated X
   connection. A callback in the test shared library has XRes's exact lifetime. */
#define dlopen test_dlopen
#define dlsym test_dlsym
#define dlclose test_dlclose
#define XCloseDisplay test_close_display
#define XUngrabKeyboard test_ungrab_keyboard
#define XQueryTree test_query_tree
#include "../native/desktop.c"
#undef dlopen
#undef dlsym
#undef dlclose
#undef XCloseDisplay
#undef XUngrabKeyboard
#undef XQueryTree
#include <assert.h>
extern void *dlopen(const char *,int);
extern void *dlsym(void *,const char *);
extern int dlclose(void *);

static const char *library_path;
static int mode,opens,closes,callbacks,display_closed;
static void (*close_hook)(Display *);
int shutdown_test_mode(void) { return mode; }
void shutdown_test_register(Display *d,void (*hook)(Display *)) { (void)d; close_hook=hook; }
void shutdown_test_callback(Display *d) { (void)d; callbacks++; }
void *test_dlopen(const char *name,int flags) {
    assert(strcmp(name,"libXRes.so.1")==0); opens++;
    return mode==3?NULL:dlopen(library_path,flags);
}
void *test_dlsym(void *library,const char *symbol) {
    if(mode==4) return NULL;
    return dlsym(library,symbol);
}
int test_dlclose(void *library) { closes++; return dlclose(library); }
int test_close_display(Display *d) {
    /* With the original code this calls into an unloaded library and crashes. */
    if(close_hook) close_hook(d);
    assert(closes==0); /* The display's hooks must precede library unloading. */
    display_closed++; return 0;
}
int test_ungrab_keyboard(Display *d,Time t) { (void)d; (void)t; return 1; }
Status test_query_tree(Display *d,Window w,Window *root,Window *parent,Window **children,unsigned int *n) {
    (void)d; (void)w; *root=1; *parent=1; *children=NULL; *n=0; return 1;
}
int main(int argc,char **argv) {
    assert(argc==3); library_path=argv[1]; mode=atoi(argv[2]);
    Desktop *x=calloc(1,sizeof(*x)); assert(x);
    x->d=(Display*)x; x->original=0x101; x->root=1;
    if(mode!=5) {
        /* Repeat the clipboard checks, like several dictations before Ctrl+C. */
        for(int i=0;i<8;i++) assert(lx_target_requestor(x,0x102)==(mode==0));
    }
    lx_close(x);
    assert(display_closed==1);
    assert(callbacks==((mode==0 || mode==1 || mode==2)?1:0));
    assert(opens==(mode==5?0:1));
    assert(closes==((mode==3 || mode==5)?0:1));
    puts("Shutdown test passed: X11 callbacks ran before unloading XRes.");
    return 0;
}
