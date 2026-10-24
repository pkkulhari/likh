/* Small Xlib ABI bridge. All calls run on the Rust desktop thread. */
#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <X11/keysym.h>
#include <X11/XKBlib.h>
#include <X11/Xresource.h>
#include <X11/Xft/Xft.h>
#include <X11/extensions/Xrandr.h>
#include <X11/extensions/shape.h>
#include <poll.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdio.h>
#include <dlfcn.h>
#include <time.h>
#include "scale.h"

extern Bool XTestFakeKeyEvent(Display*, unsigned int, Bool, unsigned long);
extern Bool XTestQueryExtension(Display*, int*, int*, int*, int*);
typedef struct {
    Display *d;
    /* XRes registers callbacks with Xlib, even when the extension is absent.
       Its code must remain loaded until XCloseDisplay has run those callbacks. */
    void *xres_library;
    int xres_load_attempted;
    Window root, window, original, snapshot_owner;
    GC gc;
    Pixmap indicator_buffer;
    int indicator_mode, indicator_width, indicator_height, shape_available;
    float indicator_scale;
    XftDraw *text_draw;
    XftFont *text_font;
    XftColor text_color;
    int text_color_allocated;
    unsigned int animation_frame;
    uint64_t next_frame_ms;
    float audio_peak, audio_envelope;
    Atom clipboard, utf8, targets, text, plain, plain_utf8, plain_utf8_upper, prop, incr, active;
    unsigned char shift_l, esc;
    unsigned int numlock, control_mask, super_mask;
    int xkb_event, hotkey_enabled, record_chord_down, escape_grabbed, changed;
    char label[128];
    unsigned char *clipboard_data, *snapshot;
    size_t clipboard_len, snapshot_len;
    int snapshot_valid;
} Desktop;
typedef struct { int kind; unsigned long window; } DesktopEvent;
static int last_error;
static int error_handler(Display *d, XErrorEvent *e) { (void)d; last_error=e->error_code; return 0; }
enum { INDICATOR_HIDDEN, INDICATOR_TEXT, INDICATOR_STARTING, INDICATOR_LISTENING, INDICATOR_PROCESSING };
enum { INDICATOR_HEIGHT=44, ACTIVITY_WIDTH=156, TEXT_WIDTH=360, FRAME_MS=33 };

static void lock_grabs(Desktop *x, int key, unsigned int base, int grab) {
    unsigned int masks[4]={base,base|LockMask,base|x->numlock,base|LockMask|x->numlock};
    for (int i=0;i<4;i++) {
        if (grab) XGrabKey(x->d,key,masks[i],x->root,False,GrabModeAsync,GrabModeAsync);
        else XUngrabKey(x->d,key,masks[i],x->root);
    }
}
static int record_chord(Desktop *x,unsigned int depressed) {
    return (depressed & x->control_mask)!=0 && (depressed & x->super_mask)!=0;
}
static int subscribe_hotkey(Desktop *x,char *error,size_t cap) {
    x->hotkey_enabled=0;
    if(!x->control_mask || !x->super_mask || (x->control_mask & x->super_mask)) {
        snprintf(error,cap,"Ctrl+Win needs distinct Ctrl and Super modifiers in the keyboard layout"); return 0;
    }
    int opcode,error_base,major=XkbMajorVersion,minor=XkbMinorVersion;
    if(!XkbQueryExtension(x->d,&opcode,&x->xkb_event,&error_base,&major,&minor)) {
        snprintf(error,cap,"XKB modifier notifications are unavailable"); return 0;
    }
    last_error=0;
    /* XKB state notifications arrive regardless of focus or keyboard grabs.
       Observe depressed modifiers only: locked/latched modifiers are not holds. */
    if(!XkbSelectEventDetails(x->d,XkbUseCoreKbd,XkbStateNotify,XkbModifierBaseMask,XkbModifierBaseMask)) {
        snprintf(error,cap,"Cannot subscribe to Ctrl+Win modifier changes"); return 0;
    }
    XkbStateRec state={0};
    if(XkbGetState(x->d,XkbUseCoreKbd,&state)!=Success) {
        snprintf(error,cap,"Cannot read the initial Ctrl+Win modifier state"); return 0;
    }
    XSync(x->d,False);
    if(last_error) { snprintf(error,cap,"Cannot enable Ctrl+Win modifier notifications"); return 0; }
    x->record_chord_down=record_chord(x,state.base_mods);
    x->hotkey_enabled=1;
    return 1;
}
static Window focus(Desktop *x) { Window w; int revert; XGetInputFocus(x->d,&w,&revert); return w; }
void lx_close(Desktop *x) {
    if (!x) return;
    if (x->d) {
        if(x->text_draw) XftDrawDestroy(x->text_draw);
        if(x->text_font) XftFontClose(x->d,x->text_font);
        if(x->text_color_allocated) XftColorFree(x->d,DefaultVisual(x->d,DefaultScreen(x->d)),
            DefaultColormap(x->d,DefaultScreen(x->d)),&x->text_color);
        if(x->indicator_buffer) XFreePixmap(x->d,x->indicator_buffer);
        XUngrabKeyboard(x->d,CurrentTime); XCloseDisplay(x->d);
    }
    if (x->xres_library) dlclose(x->xres_library);
    free(x->clipboard_data); free(x->snapshot); free(x);
}
Desktop *lx_open(int hotkey, char *error, size_t cap) {
    Desktop *x=calloc(1,sizeof(*x)); if (!x) return NULL;
    x->d=XOpenDisplay(NULL);
    if (!x->d) { snprintf(error,cap,"Cannot connect to X11 DISPLAY"); lx_close(x); return NULL; }
    XSetErrorHandler(error_handler);
    int a,b,c,d;
    if (!XTestQueryExtension(x->d,&a,&b,&c,&d)) { snprintf(error,cap,"X11 XTEST extension is unavailable"); lx_close(x); return NULL; }
    x->root=DefaultRootWindow(x->d);
    x->shape_available=XShapeQueryExtension(x->d,&a,&b);
    x->shift_l=XKeysymToKeycode(x->d,XK_Shift_L);
    x->esc=XKeysymToKeycode(x->d,XK_Escape);
    XModifierKeymap *map=XGetModifierMapping(x->d);
    if(!map) { snprintf(error,cap,"Cannot read the keyboard modifier map"); lx_close(x); return NULL; }
    KeyCode num=XKeysymToKeycode(x->d,XK_Num_Lock);
    KeyCode ctrl_l=XKeysymToKeycode(x->d,XK_Control_L),ctrl_r=XKeysymToKeycode(x->d,XK_Control_R);
    KeyCode super_l=XKeysymToKeycode(x->d,XK_Super_L),super_r=XKeysymToKeycode(x->d,XK_Super_R);
    for (int mod=0;mod<8;mod++) for(int k=0;k<map->max_keypermod;k++) {
        KeyCode code=map->modifiermap[mod*map->max_keypermod+k];
        if(!code) continue;
        if(code==num) x->numlock |= 1u<<mod;
        if(code==ctrl_l || code==ctrl_r) x->control_mask |= 1u<<mod;
        if(code==super_l || code==super_r) x->super_mask |= 1u<<mod;
    }
    XFreeModifiermap(map);
    Bool detectable=False; XkbSetDetectableAutoRepeat(x->d,True,&detectable);
    if (!detectable) { snprintf(error,cap,"XKB detectable auto-repeat is required"); lx_close(x); return NULL; }
    x->clipboard=XInternAtom(x->d,"CLIPBOARD",False); x->utf8=XInternAtom(x->d,"UTF8_STRING",False);
    x->targets=XInternAtom(x->d,"TARGETS",False); x->text=XInternAtom(x->d,"TEXT",False);
    x->plain=XInternAtom(x->d,"text/plain",False);
    x->plain_utf8=XInternAtom(x->d,"text/plain;charset=utf-8",False);
    x->plain_utf8_upper=XInternAtom(x->d,"text/plain;charset=UTF-8",False);
    x->incr=XInternAtom(x->d,"INCR",False); x->prop=XInternAtom(x->d,"_LIKH_CLIPBOARD",False);
    x->active=XInternAtom(x->d,"_NET_ACTIVE_WINDOW",False);
    XSetWindowAttributes attrs={0}; attrs.override_redirect=True; attrs.background_pixel=0x15171c;
    attrs.event_mask=ExposureMask|PropertyChangeMask;
    x->window=XCreateWindow(x->d,x->root,0,0,TEXT_WIDTH,INDICATOR_HEIGHT,0,CopyFromParent,InputOutput,CopyFromParent,
        CWOverrideRedirect|CWBackPixel|CWEventMask,&attrs);
    XStoreName(x->d,x->window,"Likh voice typing");
    XWMHints hints={0}; hints.flags=InputHint; hints.input=False; XSetWMHints(x->d,x->window,&hints);
    x->gc=XCreateGC(x->d,x->window,0,NULL); XSetForeground(x->d,x->gc,0xf1f3f5);
    XSetGraphicsExposures(x->d,x->gc,False);
    XSelectInput(x->d,x->root,PropertyChangeMask);
    if (hotkey) {
        if(!subscribe_hotkey(x,error,cap)) { lx_close(x); return NULL; }
        fprintf(stderr,"Hotkey enabled: Ctrl+Win (XKB modifier events, Ctrl=0x%x, Win=0x%x)\n",x->control_mask,x->super_mask);
    }
    XFlush(x->d); return x;
}
static uint64_t monotonic_ms(void) {
    struct timespec time; clock_gettime(CLOCK_MONOTONIC,&time);
    return (uint64_t)time.tv_sec*1000+(uint64_t)time.tv_nsec/1000000;
}
static float desktop_scale(Desktop *x) {
    float scale=scale_number(getenv("LIKH_INDICATOR_SCALE"),1);
    if(!scale) scale=scale_number(getenv("GDK_SCALE"),1);
    if(scale) return scale;
    char selection[40]; snprintf(selection,sizeof(selection),"_XSETTINGS_S%d",DefaultScreen(x->d));
    Window manager=XGetSelectionOwner(x->d,XInternAtom(x->d,selection,False));
    Atom type; int format; unsigned long count,left; unsigned char *data=NULL;
    if(manager) {
        Atom property=XInternAtom(x->d,"_XSETTINGS_SETTINGS",False);
        if(XGetWindowProperty(x->d,manager,property,0,65536,False,property,
            &type,&format,&count,&left,&data)==Success && type==property && format==8 && !left)
            scale=xsettings_scale(data,count);
        if(data) XFree(data);
        if(scale) return scale;
    }
    /* Read the current resource property so the next hold picks up DPI changes. */
    data=NULL;
    if(XGetWindowProperty(x->d,x->root,XA_RESOURCE_MANAGER,0,65536,False,XA_STRING,
        &type,&format,&count,&left,&data)==Success && type==XA_STRING && format==8 && !left && data) {
        XrmInitialize();
        XrmDatabase resources=XrmGetStringDatabase((char*)data);
        if(resources) {
            XrmValue value; char *value_type;
            if(XrmGetResource(resources,"Xft.dpi","Xft.Dpi",&value_type,&value) && value.addr)
                scale=scale_number(value.addr,96);
            XrmDestroyDatabase(resources);
        }
    }
    if(data) XFree(data);
    return scale?scale:1.0f;
}
static int pixels(Desktop *x,float logical) { return (int)(logical*x->indicator_scale+0.5f); }
/* Rounded capsules for the window, waveform bars, and microphone. */
static void capsule(Display *d,Drawable drawable,GC gc,int left,int top,int width,int height) {
    int diameter=width<height?width:height;
    if(width>height) XFillRectangle(d,drawable,gc,left+diameter/2,top,width-diameter,height);
    if(height>width) XFillRectangle(d,drawable,gc,left,top+diameter/2,width,height-diameter);
    XFillArc(d,drawable,gc,left,top,diameter,diameter,0,360*64);
    if(width!=height) XFillArc(d,drawable,gc,left+width-diameter,top+height-diameter,diameter,diameter,0,360*64);
}
static void prepare_indicator(Desktop *x,int logical_width,float scale) {
    int width=(int)(logical_width*scale+0.5f),height=(int)(INDICATOR_HEIGHT*scale+0.5f);
    if(x->indicator_width==width && x->indicator_height==height && x->indicator_scale==scale && x->indicator_buffer) return;
    if(x->text_draw) { XftDrawDestroy(x->text_draw); x->text_draw=NULL; }
    if(x->text_font && x->indicator_scale!=scale) { XftFontClose(x->d,x->text_font); x->text_font=NULL; }
    if(x->indicator_buffer) XFreePixmap(x->d,x->indicator_buffer);
    x->indicator_width=width; x->indicator_height=height; x->indicator_scale=scale;
    x->indicator_buffer=XCreatePixmap(x->d,x->window,width,height,DefaultDepth(x->d,DefaultScreen(x->d)));
    XResizeWindow(x->d,x->window,width,height);
    if(x->shape_available) {
        Pixmap mask=XCreatePixmap(x->d,x->window,width,height,1);
        GC gc=XCreateGC(x->d,mask,0,NULL);
        XSetForeground(x->d,gc,0); XFillRectangle(x->d,mask,gc,0,0,width,height);
        XSetForeground(x->d,gc,1); capsule(x->d,mask,gc,0,0,width,height);
        XShapeCombineMask(x->d,x->window,ShapeBounding,0,0,mask,ShapeSet);
        XFreeGC(x->d,gc); XFreePixmap(x->d,mask);
    }
}
static void draw_label(Desktop *x) {
    int screen=DefaultScreen(x->d);
    if(!x->text_font) x->text_font=XftFontOpen(x->d,screen,
        XFT_FAMILY,XftTypeString,"sans",XFT_PIXEL_SIZE,XftTypeDouble,13.0*x->indicator_scale,NULL);
    if(!x->text_draw) x->text_draw=XftDrawCreate(x->d,x->indicator_buffer,
        DefaultVisual(x->d,screen),DefaultColormap(x->d,screen));
    if(!x->text_color_allocated) {
        XRenderColor color={0xf1f1,0xf3f3,0xf5f5,0xffff};
        x->text_color_allocated=XftColorAllocValue(x->d,DefaultVisual(x->d,screen),
            DefaultColormap(x->d,screen),&color,&x->text_color);
    }
    if(x->text_font && x->text_draw && x->text_color_allocated) {
        int baseline=(x->indicator_height-x->text_font->ascent-x->text_font->descent)/2+x->text_font->ascent;
        XftDrawStringUtf8(x->text_draw,&x->text_color,x->text_font,pixels(x,18),baseline,
            (const FcChar8*)x->label,(int)strlen(x->label));
    } else {
        XSetForeground(x->d,x->gc,0xf1f3f5);
        XDrawString(x->d,x->indicator_buffer,x->gc,pixels(x,18),pixels(x,27),x->label,(int)strlen(x->label));
    }
}
static float pulse(unsigned int frame) {
    unsigned int phase=frame%48;
    return (float)(phase<=24?phase:48-phase)/24.0f;
}
static void draw_indicator(Desktop *x) {
    if(x->indicator_mode==INDICATOR_HIDDEN || !x->indicator_buffer) return;
    Drawable buffer=x->indicator_buffer; int width=x->indicator_width,height=x->indicator_height,border=pixels(x,1);
    XSetForeground(x->d,x->gc,0x363a42);
    XFillRectangle(x->d,buffer,x->gc,0,0,width,height);
    XSetForeground(x->d,x->gc,0x15171c);
    capsule(x->d,buffer,x->gc,border,border,width-2*border,height-2*border);
    if(x->indicator_mode==INDICATOR_TEXT) {
        draw_label(x);
    } else if(x->indicator_mode==INDICATOR_LISTENING) {
        unsigned long color=x->audio_envelope>0.06f?0x91bfff:0xe3e7ee;
        XSetForeground(x->d,x->gc,color);
        capsule(x->d,buffer,x->gc,pixels(x,23),pixels(x,12),pixels(x,6),pixels(x,11));
        XSetLineAttributes(x->d,x->gc,pixels(x,2),LineSolid,CapRound,JoinRound);
        XDrawArc(x->d,buffer,x->gc,pixels(x,20),pixels(x,16),pixels(x,12),pixels(x,12),180*64,180*64);
        XDrawLine(x->d,buffer,x->gc,pixels(x,26),pixels(x,28),pixels(x,26),pixels(x,32));
        XDrawLine(x->d,buffer,x->gc,pixels(x,22),pixels(x,32),pixels(x,30),pixels(x,32));
        XSetLineAttributes(x->d,x->gc,0,LineSolid,CapButt,JoinMiter);
        static const float shape[]={0.35f,0.5f,0.68f,0.82f,0.94f,1.0f,0.94f,0.82f,0.68f,0.5f,0.35f};
        for(int i=0;i<11;i++) {
            float motion=0.55f+0.45f*pulse(x->animation_frame+i*3);
            int bar_height=pixels(x,4+(2.0f+26.0f*x->audio_envelope)*shape[i]*motion);
            capsule(x->d,buffer,x->gc,pixels(x,53+i*7),(height-bar_height)/2,pixels(x,4),bar_height);
        }
    } else {
        /* A quiet three-dot animation while the microphone starts or text finalizes. */
        for(int i=0;i<3;i++) {
            float motion=pulse(x->animation_frame*2+i*12);
            int size=pixels(x,5+3.0f*motion);
            unsigned long color=motion>0.6f?(x->indicator_mode==INDICATOR_STARTING?0xb5bfce:0xa9bcff):0x586171;
            XSetForeground(x->d,x->gc,color);
            capsule(x->d,buffer,x->gc,pixels(x,64+i*14)-size/2,(height-size)/2,size,size);
        }
    }
    XCopyArea(x->d,buffer,x->window,x->gc,0,0,width,height,0,0);
    XFlush(x->d);
}
static void animate_indicator(Desktop *x) {
    if(x->indicator_mode<INDICATOR_STARTING) return;
    uint64_t now=monotonic_ms();
    if(now<x->next_frame_ms) return;
    x->next_frame_ms=now+FRAME_MS;
    x->animation_frame++;
    float response=x->audio_peak>x->audio_envelope?0.65f:0.18f;
    x->audio_envelope+=(x->audio_peak-x->audio_envelope)*response;
    x->audio_peak=0;
    draw_indicator(x);
}
static void show_indicator(Desktop *x) {
    prepare_indicator(x,x->indicator_mode==INDICATOR_TEXT?TEXT_WIDTH:ACTIVITY_WIDTH,desktop_scale(x));
    int width=x->indicator_width;
    int screen=DefaultScreen(x->d), px=0,py=0,wx,wy; Window rr,child; unsigned int mask;
    int mx=0,my=0,mw=DisplayWidth(x->d,screen),mh=DisplayHeight(x->d,screen);
    XQueryPointer(x->d,x->root,&rr,&child,&px,&py,&wx,&wy,&mask);
    /* Prefer the focused window's monitor; fall back to pointer location. */
    Window f=focus(x); XWindowAttributes wa;
    if(f>1 && XGetWindowAttributes(x->d,f,&wa)) XTranslateCoordinates(x->d,f,x->root,wa.width/2,wa.height/2,&px,&py,&child);
    int n=0; XRRMonitorInfo *mon=XRRGetMonitors(x->d,x->root,True,&n);
    for(int i=0;i<n;i++) if(px>=mon[i].x && px<mon[i].x+mon[i].width && py>=mon[i].y && py<mon[i].y+mon[i].height) {
        mx=mon[i].x; my=mon[i].y; mw=mon[i].width; mh=mon[i].height; break;
    }
    if(mon) XRRFreeMonitors(mon);
    XMoveWindow(x->d,x->window,mx+(mw-width)/2,my+mh-pixels(x,100));
    XMapRaised(x->d,x->window); draw_indicator(x);
}
void lx_indicator(Desktop *x,const char *label) {
    snprintf(x->label,sizeof(x->label),"%s",label);
    x->indicator_mode=*label?INDICATOR_TEXT:INDICATOR_HIDDEN;
    x->audio_peak=x->audio_envelope=0;
    if(!*label) { XUnmapWindow(x->d,x->window); XFlush(x->d); return; }
    XStoreName(x->d,x->window,"Likh status"); show_indicator(x);
}
void lx_activity(Desktop *x,int mode) {
    if(mode<INDICATOR_STARTING || mode>INDICATOR_PROCESSING) return;
    x->indicator_mode=mode; x->label[0]=0;
    x->animation_frame=0; x->audio_peak=x->audio_envelope=0;
    x->next_frame_ms=monotonic_ms()+FRAME_MS;
    const char *title=mode==INDICATOR_LISTENING?"Likh listening":mode==INDICATOR_STARTING?"Likh starting microphone":"Likh transcribing";
    XStoreName(x->d,x->window,title); show_indicator(x);
}
void lx_level(Desktop *x,float level) {
    if(x->indicator_mode!=INDICATOR_LISTENING || !(level>=0.0f)) return;
    if(level>1.0f) level=1.0f;
    if(level>x->audio_peak) x->audio_peak=level;
}
void lx_busy(Desktop *x,int busy) {
    if(busy==x->escape_grabbed) return;
    if(!x->esc) return;
    unsigned int allowed=x->control_mask|x->super_mask|ShiftMask;
    for(unsigned int mask=0;mask<256;mask++) if(!(mask & ~allowed)) lock_grabs(x,x->esc,mask,busy);
    x->escape_grabbed=busy; XFlush(x->d);
}
unsigned long lx_focus(Desktop *x) { return focus(x); }
void lx_track(Desktop *x, unsigned long w) {
    if(x->original>1) XSelectInput(x->d,x->original,NoEventMask);
    x->original=w; x->changed=0;
    if(w>1) XSelectInput(x->d,w,FocusChangeMask);
    XFlush(x->d);
}
int lx_focus_valid(Desktop *x) { return !x->changed && x->original>1 && focus(x)==x->original; }
int lx_modifiers_down(Desktop *x) {
    char keys[32]; XQueryKeymap(x->d,keys);
    XModifierKeymap *map=XGetModifierMapping(x->d); int down=0;
    for(int m=0;m<8;m++) if(m!=1 && !(x->numlock & (1u<<m))) for(int k=0;k<map->max_keypermod;k++) {
        int code=map->modifiermap[m*map->max_keypermod+k];
        if(code && (keys[code/8] & (1<<(code%8)))) down=1;
    }
    XFreeModifiermap(map);
    return down;
}
void lx_class(Desktop *x, unsigned long window, char *buf, size_t cap) {
    buf[0]=0;
    for(int i=0;i<20 && window>1 && window!=x->root;i++) {
        XClassHint hint={0};
        if(XGetClassHint(x->d,window,&hint)) {
            snprintf(buf,cap,"%s",hint.res_class?hint.res_class:"");
            if(hint.res_name) XFree(hint.res_name);
            if(hint.res_class) XFree(hint.res_class);
            return;
        }
        Window root,parent,*children=NULL; unsigned int count;
        if(!XQueryTree(x->d,window,&root,&parent,&children,&count)) break;
        if(children) XFree(children);
        window=parent;
    }
}
int lx_snapshot(Desktop *x) {
    free(x->snapshot); x->snapshot=NULL; x->snapshot_len=0; x->snapshot_valid=0;
    Window owner=XGetSelectionOwner(x->d,x->clipboard);
    x->snapshot_owner=owner;
    if(owner==x->window) {
        x->snapshot=malloc(x->clipboard_len+1); if(!x->snapshot) return -1;
        memcpy(x->snapshot,x->clipboard_data,x->clipboard_len); x->snapshot_len=x->clipboard_len; x->snapshot_valid=1; return 1;
    }
    if(owner==None) return 1;
    XDeleteProperty(x->d,x->window,x->prop);
    XConvertSelection(x->d,x->clipboard,x->utf8,x->prop,x->window,CurrentTime); XFlush(x->d); return 0;
}
int lx_set_clipboard(Desktop *x,const unsigned char *data,size_t len) {
    unsigned char *copy=malloc(len+1); if(!copy) return 0;
    memcpy(copy,data,len); copy[len]=0; free(x->clipboard_data); x->clipboard_data=copy; x->clipboard_len=len;
    XSetSelectionOwner(x->d,x->clipboard,x->window,CurrentTime); XFlush(x->d);
    return XGetSelectionOwner(x->d,x->clipboard)==x->window;
}
int lx_restore(Desktop *x) {
    if(XGetSelectionOwner(x->d,x->clipboard)!=x->window) return 0;
    if(x->snapshot_valid) return lx_set_clipboard(x,x->snapshot,x->snapshot_len);
    return 0;
}
int lx_owns_clipboard(Desktop *x) { return XGetSelectionOwner(x->d,x->clipboard)==x->window; }
int lx_snapshot_current(Desktop *x) { return XGetSelectionOwner(x->d,x->clipboard)==x->snapshot_owner; }
int lx_target_requestor(Desktop *x, unsigned long w) {
    /* XRes identifies requests from hidden toolkit windows owned by the target
       X client. Never mistake a clipboard manager's request for a completed paste.
       The library/extension is optional; otherwise use window ancestry. */
    typedef struct { XID base, mask; } ClientResource;
    if(!x->xres_load_attempted) {
        x->xres_load_attempted=1;
        x->xres_library=dlopen("libXRes.so.1",RTLD_LAZY|RTLD_LOCAL);
    }
    void *lib=x->xres_library;
    if(lib) {
        Status (*query)(Display*,int*,ClientResource**) = (Status(*)(Display*,int*,ClientResource**))dlsym(lib,"XResQueryClients");
        Bool (*extension)(Display*,int*,int*) = (Bool(*)(Display*,int*,int*))dlsym(lib,"XResQueryExtension");
        int a,b,n=0; ClientResource *clients=NULL;
        if(query && extension && extension(x->d,&a,&b) && query(x->d,&n,&clients)) {
            int same=0;
            for(int i=0;i<n;i++) if((w & ~clients[i].mask)==clients[i].base && (x->original & ~clients[i].mask)==clients[i].base) same=1;
            XFree(clients); return same;
        }
    }
    for(int i=0;i<30 && w>1 && w!=x->root;i++) {
        if(w==x->original) return 1;
        Window root,parent,*children=NULL; unsigned int n;
        if(!XQueryTree(x->d,w,&root,&parent,&children,&n)) break;
        if(children) XFree(children);
        w=parent;
    }
    return 0;
}
int lx_paste(Desktop *x,int shortcut) {
    if(!lx_focus_valid(x) || lx_modifiers_down(x) || !lx_owns_clipboard(x)) return 0;
    KeyCode ctrl=XKeysymToKeycode(x->d,XK_Control_L), shift=x->shift_l;
    KeyCode key=XKeysymToKeycode(x->d,shortcut==2?XK_Insert:XK_v);
    if(!key || !ctrl || !shift) return 0;
    /* End any active Escape grab before injecting the paste shortcut. */
    XUngrabKeyboard(x->d,CurrentTime);
    if(shortcut!=2) XTestFakeKeyEvent(x->d,ctrl,True,0);
    if(shortcut!=0) XTestFakeKeyEvent(x->d,shift,True,0);
    XTestFakeKeyEvent(x->d,key,True,0); XTestFakeKeyEvent(x->d,key,False,0);
    if(shortcut!=0) XTestFakeKeyEvent(x->d,shift,False,0);
    if(shortcut!=2) XTestFakeKeyEvent(x->d,ctrl,False,0);
    XFlush(x->d); return 1;
}
/* STRING is Latin-1, not UTF-8. Do not advertise a conversion we cannot serve. */
static unsigned char *latin1_text(const unsigned char *utf8,size_t len,size_t *out_len) {
    unsigned char *result=malloc(len+1);
    if(!result) return NULL;
    size_t n=0;
    for(size_t i=0;i<len;i++) {
        unsigned char c=utf8[i];
        if(c<0x80) result[n++]=c;
        else if((c==0xc2 || c==0xc3) && i+1<len && (utf8[i+1]&0xc0)==0x80) {
            result[n++]=(unsigned char)(((c&0x1f)<<6)|(utf8[++i]&0x3f));
        } else { free(result); return NULL; }
    }
    result[n]=0; *out_len=n; return result;
}
static void selection_request(Desktop *x,XSelectionRequestEvent *r,DesktopEvent *out) {
    XSelectionEvent notify={0}; notify.type=SelectionNotify; notify.display=x->d; notify.requestor=r->requestor;
    notify.selection=r->selection; notify.target=r->target; notify.time=r->time; notify.property=None;
    Atom prop=r->property?r->property:r->target;
    if(r->selection==x->clipboard && x->clipboard_data && lx_owns_clipboard(x)) {
        if(r->target==x->targets) {
            Atom targets[]={x->targets,x->utf8,x->text,x->plain,x->plain_utf8,x->plain_utf8_upper,XA_STRING};
            size_t len=0; unsigned char *latin=latin1_text(x->clipboard_data,x->clipboard_len,&len);
            int count=latin?7:6; free(latin);
            XChangeProperty(x->d,r->requestor,prop,XA_ATOM,32,PropModeReplace,(unsigned char*)targets,count); notify.property=prop;
        } else if(r->target==x->utf8 || r->target==x->text || r->target==x->plain || r->target==x->plain_utf8 || r->target==x->plain_utf8_upper) {
            Atom type=r->target==x->text?x->utf8:r->target;
            XChangeProperty(x->d,r->requestor,prop,type,8,PropModeReplace,x->clipboard_data,(int)x->clipboard_len);
            notify.property=prop; out->kind=4; out->window=r->requestor;
        } else if(r->target==XA_STRING) {
            size_t len=0; unsigned char *latin=latin1_text(x->clipboard_data,x->clipboard_len,&len);
            if(latin) {
                XChangeProperty(x->d,r->requestor,prop,XA_STRING,8,PropModeReplace,latin,(int)len);
                free(latin); notify.property=prop; out->kind=4; out->window=r->requestor;
            }
        }
    }
    XSendEvent(x->d,r->requestor,False,0,(XEvent*)&notify); XFlush(x->d);
}
int lx_next(Desktop *x,int timeout_ms,DesktopEvent *out) {
    out->kind=0; out->window=0;
    animate_indicator(x);
    if(x->indicator_mode>=INDICATOR_STARTING && timeout_ms>FRAME_MS) timeout_ms=FRAME_MS;
    if(!XPending(x->d)) { struct pollfd fd={ConnectionNumber(x->d),POLLIN,0}; poll(&fd,1,timeout_ms); }
    animate_indicator(x);
    if(!XPending(x->d)) return 0;
    XEvent e; XNextEvent(x->d,&e);
    if(e.type==Expose) draw_indicator(x);
    else if(x->hotkey_enabled && e.type==x->xkb_event) {
        XkbEvent *kb=(XkbEvent*)&e;
        if(kb->any.xkb_type==XkbStateNotify) {
            int down=record_chord(x,kb->state.base_mods);
            if(down!=x->record_chord_down) {
                x->record_chord_down=down; out->kind=down?1:2;
            }
        }
    } else if(e.type==KeyPress && e.xkey.keycode==x->esc && x->escape_grabbed) out->kind=3;
    else if(e.type==SelectionRequest) selection_request(x,&e.xselectionrequest,out);
    else if(e.type==SelectionNotify && e.xselection.selection==x->clipboard) {
        if(e.xselection.property!=None) {
            Atom type; int format; unsigned long n,remain; unsigned char *data=NULL;
            if(XGetWindowProperty(x->d,x->window,x->prop,0,32768,True,AnyPropertyType,&type,&format,&n,&remain,&data)==Success) {
                if(type==x->utf8 && format==8 && !remain) {
                    x->snapshot=malloc(n+1);
                    if(x->snapshot) { memcpy(x->snapshot,data,n); x->snapshot_len=n; x->snapshot_valid=1; }
                }
                if(data) XFree(data);
            }
        }
        out->kind=5;
    } else if(e.type==FocusOut && e.xfocus.window==x->original && e.xfocus.mode==NotifyNormal) x->changed=1;
    else if(e.type==MappingNotify) XRefreshKeyboardMapping(&e.xmapping);
    return 1;
}
