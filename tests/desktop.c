/* Run on a private Xvfb server, never on the user's desktop. */
#include "../native/desktop.c"
#include <assert.h>
#include <time.h>

static void key(Display *d, KeySym sym, int down) {
    XTestFakeKeyEvent(d,XKeysymToKeycode(d,sym),down,0); XSync(d,False);
}
static int wait_kind(Desktop *x,int kind) {
    for(int i=0;i<100;i++) { DesktopEvent e; lx_next(x,10,&e); if(e.kind==kind) return 1; }
    return 0;
}
static void drain(Desktop *x) { for(int i=0;i<20;i++) { DesktopEvent e; lx_next(x,1,&e); assert(e.kind!=1 && e.kind!=2); } }
static void read_clip(Desktop *x,Display *d,Window w,const char *expected) {
    Atom prop=XInternAtom(d,"TEST_CLIP",False);
    XConvertSelection(d,x->clipboard,x->utf8,prop,w,CurrentTime); XFlush(d);
    int received=0;
    for(int i=0;i<100 && !received;i++) {
        DesktopEvent out; lx_next(x,5,&out);
        while(XPending(d)) {
            XEvent e; XNextEvent(d,&e);
            if(e.type==SelectionNotify) {
                Atom type; int format; unsigned long n,left; unsigned char *bytes=NULL;
                assert(e.xselection.property==prop);
                XGetWindowProperty(d,w,prop,0,32768,True,AnyPropertyType,&type,&format,&n,&left,&bytes);
                assert(format==8 && !left && n==strlen(expected)); assert(memcmp(bytes,expected,n)==0);
                XFree(bytes); received=1;
            }
        }
    }
    assert(received);
}
int main(void) {
    unsetenv("GDK_SCALE");
    setenv("LIKH_INDICATOR_SCALE","1",1);
    Display *client=XOpenDisplay(NULL);
    if(!client) { fputs("Cannot connect to the test X11 display; desktop integration tests did not run.\n",stderr); return 1; }
    Window w=XCreateSimpleWindow(client,DefaultRootWindow(client),20,20,400,200,0,0,0);
    XSelectInput(client,w,KeyPressMask|KeyReleaseMask); XMapWindow(client,w);
    XSetInputFocus(client,w,RevertToParent,CurrentTime); XSync(client,False);
    char error[256]={0}; Desktop *x=lx_open(1,error,sizeof(error));
    if(!x) { fprintf(stderr,"%s\n",error); return 1; }
    lx_track(x,w); assert(lx_focus_valid(x));
    /* Modifier observation does not compete for exclusive key grabs. */
    Desktop *observer=lx_open(1,error,sizeof(error)); assert(observer); lx_close(observer);
    const KeySym controls[]={XK_Control_L,XK_Control_R};
    const KeySym supers[]={XK_Super_L,XK_Super_R};
    for(int c=0;c<2;c++) for(int wkey=0;wkey<2;wkey++)
        for(int order=0;order<2;order++) for(int release=0;release<2;release++) {
            KeySym first=order?supers[wkey]:controls[c],second=order?controls[c]:supers[wkey];
            key(client,first,1); drain(x); /* a single modifier does not start */
            key(client,second,1); assert(wait_kind(x,1)); assert(lx_modifiers_down(x));
            key(client,release?supers[wkey]:controls[c],0); assert(wait_kind(x,2));
            assert(lx_modifiers_down(x));
            key(client,release?controls[c]:supers[wkey],0); drain(x); assert(!lx_modifiers_down(x));
        }
    /* Ordinary modifier events still reach the focused input and stay balanced. */
    int presses[256]={0},saw_ctrl=0,saw_super=0;
    while(XPending(client)) {
        XEvent e; XNextEvent(client,&e);
        if(e.type==KeyPress || e.type==KeyRelease) {
            assert(e.xkey.keycode<256);
            presses[e.xkey.keycode]+=e.type==KeyPress?1:-1;
            if(e.xkey.keycode==XKeysymToKeycode(client,XK_Control_L)) saw_ctrl=1;
            if(e.xkey.keycode==XKeysymToKeycode(client,XK_Super_L)) saw_super=1;
        }
    }
    assert(saw_ctrl && saw_super);
    for(int i=0;i<256;i++) assert(presses[i]==0);
    /* XKB notification delivery must survive another client's active grab. */
    assert(XGrabKeyboard(client,w,False,GrabModeAsync,GrabModeAsync,CurrentTime)==GrabSuccess);
    key(client,XK_Control_L,1); drain(x);
    key(client,XK_Super_L,1); assert(wait_kind(x,1));
    key(client,XK_Control_L,0); assert(wait_kind(x,2)); key(client,XK_Super_L,0); drain(x);
    XUngrabKeyboard(client,CurrentTime); XSync(client,False);
    /* Locks do not activate the chord, or prevent real holds from working. */
    const unsigned int states[]={LockMask,Mod2Mask,LockMask|Mod2Mask,ControlMask|Mod4Mask,Mod3Mask|Mod5Mask};
    for(size_t i=0;i<sizeof(states)/sizeof(states[0]);i++) {
        assert(XkbLockModifiers(client,XkbUseCoreKbd,0xff,states[i])); XSync(client,False); drain(x);
        key(client,XK_Control_R,1); drain(x);
        key(client,XK_Super_R,1); assert(wait_kind(x,1));
        key(client,XK_Control_R,0); assert(wait_kind(x,2)); key(client,XK_Super_R,0); drain(x);
    }
    assert(XkbLockModifiers(client,XkbUseCoreKbd,0xff,0)); XSync(client,False);
    /* Holding both Ctrl keys keeps Ctrl active until the last one is released. */
    key(client,XK_Control_L,1); key(client,XK_Control_R,1); drain(x);
    key(client,XK_Super_L,1); assert(wait_kind(x,1));
    key(client,XK_Control_L,0); drain(x);
    key(client,XK_Control_R,0); assert(wait_kind(x,2)); key(client,XK_Super_L,0); drain(x);
    /* Escape cancels while both modifiers are still held; repeats do not restart. */
    key(client,XK_Control_L,1); key(client,XK_Super_L,1); assert(wait_kind(x,1)); lx_busy(x,1);
    key(client,XK_Escape,1); assert(wait_kind(x,3)); key(client,XK_Escape,0); lx_busy(x,0);
    key(client,XK_Shift_L,1); key(client,XK_Shift_L,0); drain(x);
    key(client,XK_Control_L,0); assert(wait_kind(x,2)); key(client,XK_Super_L,0); drain(x);
    /* Escape also cancels after either release order while the other key is held. */
    for(int release=0;release<2;release++) {
        key(client,XK_Control_L,1); key(client,XK_Super_L,1); assert(wait_kind(x,1)); lx_busy(x,1);
        key(client,release?XK_Super_L:XK_Control_L,0); assert(wait_kind(x,2));
        key(client,XK_Escape,1); assert(wait_kind(x,3)); key(client,XK_Escape,0); lx_busy(x,0);
        key(client,release?XK_Control_L:XK_Super_L,0); drain(x);
    }
    /* Both Ctrl keys remain available for ordinary shortcuts. */
    while(XPending(client)) { XEvent e; XNextEvent(client,&e); }
    for(int c=0;c<2;c++) {
        key(client,controls[c],1); key(client,XK_c,1); key(client,XK_c,0); key(client,controls[c],0); drain(x);
        int ctrl_c=0;
        while(XPending(client)) {
            XEvent e; XNextEvent(client,&e);
            if(e.type==KeyPress && e.xkey.keycode==XKeysymToKeycode(client,XK_c)) {
                assert(e.xkey.state&ControlMask); ctrl_c=1;
            }
        }
        assert(ctrl_c);
    }
    key(client,XK_Shift_L,1); key(client,XK_space,1); key(client,XK_space,0); key(client,XK_Shift_L,0); drain(x);
    lx_busy(x,1); key(client,XK_Escape,1); assert(wait_kind(x,3)); key(client,XK_Escape,0); lx_busy(x,0);
    lx_track(x,w);
    lx_activity(x,INDICATOR_STARTING); assert(lx_focus_valid(x));
    XWindowAttributes indicator;
    assert(XGetWindowAttributes(client,x->window,&indicator));
    assert(indicator.width==ACTIVITY_WIDTH && indicator.height==INDICATOR_HEIGHT);
    lx_activity(x,INDICATOR_LISTENING); assert(!x->label[0]);
    lx_level(x,0.8f); x->next_frame_ms=0;
    DesktopEvent meter_event; lx_next(x,0,&meter_event);
    assert(x->audio_envelope>0.0f && lx_focus_valid(x));
    lx_activity(x,INDICATOR_PROCESSING); assert(x->audio_envelope==0.0f);
    lx_indicator(x,"Cancelled"); assert(lx_focus_valid(x));
    assert(XGetWindowAttributes(client,x->window,&indicator)); assert(indicator.width==TEXT_WIDTH);
    lx_indicator(x,""); unsigned int last_frame=x->animation_frame;
    x->next_frame_ms=0; lx_next(x,0,&meter_event);
    assert(x->animation_frame==last_frame && x->indicator_mode==INDICATOR_HIDDEN);
    /* Emulate a desktop advertising 200% scaling through XSETTINGS. */
    unsigned char settings[48]={0};
    settings[8]=1; settings[14]=23;
    memcpy(settings+16,"Gdk/WindowScalingFactor",23); settings[44]=2;
    Atom settings_property=XInternAtom(client,"_XSETTINGS_SETTINGS",False);
    Atom settings_selection=XInternAtom(client,"_XSETTINGS_S0",False);
    XChangeProperty(client,w,settings_property,settings_property,8,PropModeReplace,settings,sizeof(settings));
    XSetSelectionOwner(client,settings_selection,w,CurrentTime); XSync(client,False);
    unsetenv("LIKH_INDICATOR_SCALE");
    lx_activity(x,INDICATOR_LISTENING); assert(lx_focus_valid(x));
    assert(XGetWindowAttributes(client,x->window,&indicator));
    assert(indicator.width==312 && indicator.height==88 && x->indicator_scale==2.0f);
    lx_indicator(x,"Cancelled"); assert(x->text_font && x->text_draw && x->text_color_allocated);
    assert(XGetWindowAttributes(client,x->window,&indicator));
    assert(indicator.width==720 && indicator.height==88);
    assert(x->text_font->height>20); /* Font scales along with the pill. */
    setenv("LIKH_INDICATOR_SCALE","1.5",1); lx_activity(x,INDICATOR_PROCESSING);
    assert(XGetWindowAttributes(client,x->window,&indicator));
    assert(indicator.width==234 && indicator.height==66 && !x->text_font);
    lx_indicator(x,"");
    const char *unicode="Hello, नमस्ते — café.";
    assert(lx_set_clipboard(x,(const unsigned char*)"before",6)); assert(lx_snapshot(x)==1);
    assert(lx_snapshot_current(x)); assert(lx_set_clipboard(x,(const unsigned char*)unicode,strlen(unicode)));
    key(client,XK_Control_L,1); key(client,XK_Super_L,1); assert(wait_kind(x,1)); assert(!lx_paste(x,0));
    key(client,XK_Control_L,0); assert(wait_kind(x,2)); assert(!lx_paste(x,0));
    key(client,XK_Super_L,0); drain(x);
    assert(lx_paste(x,0)); read_clip(x,client,w,unicode);
    drain(x); /* Synthetic Left Ctrl paste must not activate recording. */
    /* Zed's shared editor/terminal paste must send Shift+Insert, never Ctrl+V. */
    while(XPending(client)) { XEvent e; XNextEvent(client,&e); }
    assert(lx_paste(x,2));
    int insert=0;
    for(int i=0;i<100 && !insert;i++) {
        XSync(client,False);
        while(XPending(client)) {
            XEvent e; XNextEvent(client,&e);
            if(e.type==KeyPress && e.xkey.keycode==XKeysymToKeycode(client,XK_Insert)) {
                assert((e.xkey.state&ShiftMask) && !(e.xkey.state&ControlMask)); insert=1;
            }
            if(e.type==KeyPress) assert(e.xkey.keycode!=XKeysymToKeycode(client,XK_v));
        }
        struct timespec pause={0,1000000}; nanosleep(&pause,NULL);
    }
    assert(insert);
    read_clip(x,client,w,unicode);
    assert(lx_target_requestor(x,w)); assert(lx_restore(x)); read_clip(x,client,w,"before");
    XSetSelectionOwner(client,x->clipboard,w,CurrentTime); XSync(client,False);
    assert(!lx_restore(x)); assert(XGetSelectionOwner(client,x->clipboard)==w);
    Window other=XCreateSimpleWindow(client,DefaultRootWindow(client),0,0,100,100,0,0,0);
    XMapWindow(client,other); XSetInputFocus(client,other,RevertToParent,CurrentTime); XSync(client,False);
    assert(!lx_focus_valid(x)); assert(!lx_paste(x,0));
    lx_close(x); XCloseDisplay(client);
    puts("Desktop tests passed: hotkey/repeats/releases/locks, Escape, focus, Unicode clipboard, restore, ownership.");
    return 0;
}
