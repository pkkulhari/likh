/* Exercise production XKB hold detection and paste injection without a display. */
#define XPending test_pending
#define XNextEvent test_next_event
#define XQueryKeymap test_query_keymap
#define XGetModifierMapping test_modifier_mapping
#define XFreeModifiermap test_free_modifier_mapping
#define XKeysymToKeycode test_keycode
#define XGetInputFocus test_input_focus
#define XGetSelectionOwner test_selection_owner
#define XUngrabKeyboard test_ungrab_keyboard
#define XTestFakeKeyEvent test_fake_key
#define XFlush test_flush
#define XGrabKey test_grab_key
#define XUngrabKey test_ungrab_key
#define XSync test_sync
#define XkbQueryExtension test_xkb_query
#define XkbSelectEventDetails test_xkb_select
#define XkbGetState test_xkb_state
#include "../native/desktop.c"
#include <assert.h>

enum { CTRL_L=37, CTRL_R=105, SUPER_L=133, SUPER_R=134, SHIFT_L=50, SPACE=65, ESC=9, V=55, INSERT=118 };
static XEvent pending;
static char held[32];
static KeyCode modifiers[16]={SHIFT_L,0,0,0,CTRL_L,CTRL_R,0,0,0,0,0,0,SUPER_L,SUPER_R,0,0};
static XModifierKeymap modifier_map={2,modifiers};
static unsigned int injected_keys[8];
static Bool injected_down[8];
static int injected_count,ungrabs,backend_mode,subscriptions;
static unsigned int initial_base;
static int escape_masks[256];

Bool test_xkb_query(Display *d,int *opcode,int *event_base,int *error_base,int *major,int *minor) {
    (void)d; assert(*major==XkbMajorVersion && *minor==XkbMinorVersion);
    *opcode=135; *event_base=100; *error_base=150; return backend_mode!=1;
}
Bool test_xkb_select(Display *d,unsigned int device,unsigned int type,unsigned long affect,unsigned long details) {
    (void)d; assert(device==XkbUseCoreKbd && type==XkbStateNotify);
    assert(affect==XkbModifierBaseMask && details==XkbModifierBaseMask);
    subscriptions++; return backend_mode!=2;
}
Status test_xkb_state(Display *d,unsigned int device,XkbStatePtr state) {
    (void)d; assert(device==XkbUseCoreKbd); memset(state,0,sizeof(*state));
    state->base_mods=initial_base; return backend_mode==3?BadAccess:Success;
}
int test_grab_key(Display *d,int key,unsigned int mods,Window w,Bool owner,int pointer_mode,int keyboard_mode) {
    (void)d; (void)w;
    /* Only Escape is grabbed; activation must not grab either modifier. */
    assert(key==ESC && !owner && pointer_mode==GrabModeAsync && keyboard_mode==GrabModeAsync);
    assert(mods<256); escape_masks[mods]=1; return 1;
}
int test_ungrab_key(Display *d,int key,unsigned int mods,Window w) {
    (void)d; (void)w; assert(key==ESC && mods<256); escape_masks[mods]=0; return 1;
}
int test_sync(Display *d,Bool discard) { (void)d; assert(!discard); last_error=backend_mode==4?BadAccess:0; return 1; }
int test_pending(Display *d) { (void)d; return 1; }
int test_next_event(Display *d,XEvent *event) { (void)d; *event=pending; return 0; }
int test_query_keymap(Display *d,char keys[32]) { (void)d; memcpy(keys,held,32); return 1; }
XModifierKeymap *test_modifier_mapping(Display *d) { (void)d; return &modifier_map; }
int test_free_modifier_mapping(XModifierKeymap *map) { assert(map==&modifier_map); return 1; }
KeyCode test_keycode(Display *d,KeySym sym) {
    (void)d;
    switch(sym) {
        case XK_Control_L: return CTRL_L;
        case XK_Shift_L: return SHIFT_L;
        case XK_v: return V;
        case XK_Insert: return INSERT;
        default: assert(0); return 0;
    }
}
int test_input_focus(Display *d,Window *w,int *revert) { (void)d; *w=42; *revert=RevertToParent; return 1; }
Window test_selection_owner(Display *d,Atom selection) { (void)d; (void)selection; return 43; }
int test_ungrab_keyboard(Display *d,Time time) { (void)d; (void)time; ungrabs++; return 1; }
Bool test_fake_key(Display *d,unsigned int key,Bool down,unsigned long delay) {
    (void)d; (void)delay; assert(injected_count<8);
    injected_keys[injected_count]=key; injected_down[injected_count++]=down; return True;
}
int test_flush(Display *d) { (void)d; return 1; }
static void deliver(Desktop *x,int expected) {
    DesktopEvent out={0}; assert(lx_next(x,0,&out)); assert(out.kind==expected);
}
static void key_event(Desktop *x,int type,unsigned int code,int expected) {
    memset(&pending,0,sizeof(pending)); pending.type=type; pending.xkey.keycode=code;
    if(type==KeyPress) held[code/8]|=1<<(code%8);
    else held[code/8]&=~(1<<(code%8));
    deliver(x,expected);
}
static void state_event(Desktop *x,unsigned int base,unsigned int latched,unsigned int locked,int expected) {
    memset(&pending,0,sizeof(pending)); XkbEvent *kb=(XkbEvent*)&pending;
    kb->state.type=100; kb->state.xkb_type=XkbStateNotify; kb->state.changed=XkbModifierBaseMask;
    kb->state.base_mods=base; kb->state.latched_mods=latched; kb->state.locked_mods=locked;
    kb->state.mods=base|latched|locked; deliver(x,expected);
}
int main(void) {
    const unsigned int chord=ControlMask|Mod4Mask;
    Desktop x={0}; x.shift_l=SHIFT_L; x.esc=ESC; x.original=42; x.window=43;
    x.numlock=Mod2Mask; x.control_mask=ControlMask; x.super_mask=Mod4Mask;
    char error[256]={0};
    x.super_mask=0; assert(!subscribe_hotkey(&x,error,sizeof(error))); assert(!subscriptions);
    x.super_mask=ControlMask; assert(!subscribe_hotkey(&x,error,sizeof(error)));
    x.super_mask=Mod4Mask;
    for(backend_mode=1;backend_mode<=4;backend_mode++) {
        assert(!subscribe_hotkey(&x,error,sizeof(error))); assert(!x.hotkey_enabled);
    }
    backend_mode=0; assert(subscribe_hotkey(&x,error,sizeof(error)));
    /* Core modifier presses, repeats and unrelated keys cannot activate. */
    key_event(&x,KeyPress,CTRL_R,0); key_event(&x,KeyRelease,CTRL_R,0);
    key_event(&x,KeyPress,SUPER_L,0); key_event(&x,KeyRelease,SUPER_L,0);
    state_event(&x,0,chord,0,0); state_event(&x,0,0,chord,0);
    state_event(&x,ControlMask,0,Mod4Mask,0);
    state_event(&x,0,0,0,0);
    /* Both press orders and both release orders, with arbitrary lock states. */
    const unsigned int first[]={ControlMask,Mod4Mask};
    for(int press=0;press<2;press++) for(int release=0;release<2;release++) {
        state_event(&x,first[press],0,LockMask|Mod2Mask,0);
        state_event(&x,chord,0,LockMask|Mod2Mask,1);
        state_event(&x,chord,0,0,0); /* repeated state is not another start */
        state_event(&x,chord|ShiftMask,0,0,0);
        state_event(&x,first[release],0,0,2);
        state_event(&x,0,0,0,0);
    }
    /* Once cancelled, holding the chord (or toggling Shift) cannot restart it. */
    state_event(&x,chord,0,0,1); lx_busy(&x,1);
    assert(escape_masks[0] && escape_masks[ControlMask] && escape_masks[Mod4Mask]);
    assert(escape_masks[chord] && escape_masks[chord|LockMask|Mod2Mask|ShiftMask]);
    key_event(&x,KeyPress,ESC,3); key_event(&x,KeyRelease,ESC,0); lx_busy(&x,0);
    for(int i=0;i<256;i++) assert(!escape_masks[i]);
    state_event(&x,chord,0,0,0); state_event(&x,chord|ShiftMask,0,0,0);
    state_event(&x,Mod4Mask,0,0,2); state_event(&x,0,0,0,0);
    state_event(&x,chord,0,0,1); state_event(&x,0,0,0,2);
    /* Starting with keys held waits for a fresh chord before a start event. */
    initial_base=chord; assert(subscribe_hotkey(&x,error,sizeof(error)));
    state_event(&x,chord,0,0,0); state_event(&x,0,0,0,2); initial_base=0;
    /* Unrelated XKB messages and an unsubscribed desktop cannot activate. */
    memset(&pending,0,sizeof(pending)); pending.type=100;
    ((XkbEvent*)&pending)->any.xkb_type=XkbMapNotify; deliver(&x,0);
    x.hotkey_enabled=0; state_event(&x,chord,0,0,0); x.hotkey_enabled=1;
    /* Paste waits for both modifiers, including either right-hand key. */
    key_event(&x,KeyPress,CTRL_R,0); key_event(&x,KeyPress,SUPER_R,0);
    assert(!lx_paste(&x,0)); key_event(&x,KeyRelease,CTRL_R,0);
    assert(!lx_paste(&x,0)); key_event(&x,KeyRelease,SUPER_R,0);
    assert(!lx_modifiers_down(&x)); assert(!injected_count);
    const unsigned int sequences[3][6]={{CTRL_L,V,V,CTRL_L},{CTRL_L,SHIFT_L,V,V,SHIFT_L,CTRL_L},{SHIFT_L,INSERT,INSERT,SHIFT_L}};
    const int counts[3]={4,6,4};
    for(int shortcut=0;shortcut<3;shortcut++) {
        injected_count=0; assert(lx_paste(&x,shortcut)); assert(injected_count==counts[shortcut]);
        for(int i=0;i<injected_count;i++) {
            assert(injected_keys[i]==sequences[shortcut][i]);
            assert(injected_down[i]==(i<counts[shortcut]/2));
            key_event(&x,injected_down[i]?KeyPress:KeyRelease,injected_keys[i],0);
        }
        state_event(&x,shortcut==2?ShiftMask:ControlMask,0,0,0); state_event(&x,0,0,0,0);
        assert(!lx_modifiers_down(&x));
    }
    assert(ungrabs==3);
    puts("Hotkey tests passed: XKB subscription, Ctrl+Win orders, locks, cancellation, release-before-paste and injection.");
    return 0;
}
