/* Test selection conversion without an X server, mocking only Xlib transport. */
#define XChangeProperty test_change_property
#define XGetSelectionOwner test_selection_owner
#define XSendEvent test_send_event
#define XFlush test_flush
#include "../native/desktop.c"
#include <assert.h>

static Window owner=42;
static Atom result_type, result_property;
static int result_format, result_count;
static unsigned char result_bytes[1024];
static unsigned long result_atoms[32];
static XSelectionEvent notification;
int test_change_property(Display *d,Window w,Atom prop,Atom type,int format,int mode,const unsigned char *data,int count) {
    (void)d; (void)w; assert(mode==PropModeReplace);
    result_property=prop; result_type=type; result_format=format; result_count=count;
    if(format==8) { assert(count<1024); memcpy(result_bytes,data,count); }
    else { assert(format==32 && count<32); memcpy(result_atoms,data,count*sizeof(unsigned long)); }
    return 1;
}
Window test_selection_owner(Display *d,Atom selection) { (void)d; (void)selection; return owner; }
int test_send_event(Display *d,Window w,Bool propagate,long mask,XEvent *event) {
    (void)d; (void)w; (void)propagate; (void)mask; notification=event->xselection; return 1;
}
int test_flush(Display *d) { (void)d; return 1; }
static DesktopEvent request(Desktop *x,Atom target,Atom property) {
    result_count=-1; memset(&notification,0,sizeof(notification));
    XSelectionRequestEvent r={0}; r.selection=x->clipboard; r.target=target; r.property=property; r.requestor=99;
    DesktopEvent event={0}; selection_request(x,&r,&event);
    assert(notification.type==SelectionNotify && notification.requestor==99 && notification.target==target);
    return event;
}
static int advertised(Atom target) {
    for(int i=0;i<result_count;i++) if(result_atoms[i]==target) return 1;
    return 0;
}
int main(void) {
    Desktop x={0}; x.window=42; x.clipboard=100; x.utf8=101; x.targets=102; x.text=103;
    x.plain=104; x.plain_utf8=105; x.plain_utf8_upper=106;
    const char *unicode="Hello, नमस्ते — café.";
    x.clipboard_data=(unsigned char*)unicode; x.clipboard_len=strlen(unicode);
    request(&x,x.targets,200);
    assert(result_type==XA_ATOM && result_format==32 && result_count==6);
    Atom formats[]={x.utf8,x.text,x.plain,x.plain_utf8,x.plain_utf8_upper};
    for(size_t i=0;i<sizeof(formats)/sizeof(formats[0]);i++) assert(advertised(formats[i]));
    assert(!advertised(XA_STRING));
    for(size_t i=0;i<sizeof(formats)/sizeof(formats[0]);i++) {
        DesktopEvent event=request(&x,formats[i],200);
        assert(notification.property==200 && result_property==200 && result_format==8);
        assert(result_type==(formats[i]==x.text?x.utf8:formats[i]));
        assert(result_count==(int)strlen(unicode) && memcmp(result_bytes,unicode,result_count)==0);
        assert(event.kind==4 && event.window==99);
    }
    request(&x,x.plain_utf8,None); assert(notification.property==x.plain_utf8);
    request(&x,999,200); assert(notification.property==None && result_count==-1);
    request(&x,XA_STRING,200); assert(notification.property==None && result_count==-1);
    x.clipboard_data=(unsigned char*)"café £"; x.clipboard_len=strlen((char*)x.clipboard_data);
    request(&x,x.targets,200); assert(advertised(XA_STRING));
    request(&x,XA_STRING,200);
    const unsigned char latin[]={'c','a','f',0xe9,' ',0xa3};
    assert(result_type==XA_STRING && result_count==6 && !memcmp(result_bytes,latin,6));
    owner=123; request(&x,x.utf8,200); assert(notification.property==None && result_count==-1);
    puts("Clipboard tests passed: UTF-8 MIME formats, property types, Latin-1, unsupported targets, ownership.");
    return 0;
}
