/* Simulate XRes registering an Xlib close-display hook in a shared library. */
#include <X11/Xlib.h>
#include <stdlib.h>

extern int shutdown_test_mode(void);
extern void shutdown_test_register(Display *, void (*)(Display *));
extern void shutdown_test_callback(Display *);
static void close_display(Display *d) { shutdown_test_callback(d); }

Bool XResQueryExtension(Display *d,int *event,int *error) {
    shutdown_test_register(d,close_display);
    *event=0; *error=0;
    return shutdown_test_mode()!=1;
}
Status XResQueryClients(Display *d,int *count,void **clients) {
    (void)d;
    if(shutdown_test_mode()==2) { *count=0; *clients=NULL; return 0; }
    unsigned long *resources=malloc(2*sizeof(unsigned long));
    if(!resources) return 0;
    resources[0]=0x100; resources[1]=0xff;
    *count=1; *clients=resources;
    return 1;
}
