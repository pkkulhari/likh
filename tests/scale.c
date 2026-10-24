#include "../native/scale.h"
#include <assert.h>
#include <stdio.h>

static unsigned char data[512];
static size_t used;
static int big;
static void number(uint32_t value,int size) {
    for(int i=0;i<size;i++) data[used++]=(unsigned char)(value>>(8*(big?size-1-i:i)));
}
static void padding(void) { while(used%4) data[used++]=0; }
static void header(int type,const char *name) {
    number(type,1); number(0,1); number((uint32_t)strlen(name),2);
    memcpy(data+used,name,strlen(name)); used+=strlen(name); padding(); number(0,4);
}
static void begin(int count) {
    used=0; number(big,1); padding(); number(1,4); number(count,4);
}
int main(void) {
    assert(scale_number("2",1)==2 && scale_number("2.25",1)==2.25f);
    assert(scale_number("192\n",96)==2 && scale_number(NULL,1)==0);
    const char *invalid[]={""," ","0","-2","0.5","4.1","2oops","NaN","Inf","1e99"};
    for(size_t i=0;i<sizeof(invalid)/sizeof(*invalid);i++) assert(scale_number(invalid[i],1)==0);
    for(big=0;big<=1;big++) {
        /* Both wire orders, padded names and strings, and unrelated color settings. */
        begin(4);
        header(1,"Net/ThemeName"); number(3,4); memcpy(data+used,"abc",3); used+=3; padding();
        header(2,"Gtk/Color"); number(0,4); number(0xffffffff,4);
        header(0,"Xft/DPI"); number(288*1024,4);
        header(0,"Gdk/WindowScalingFactor"); size_t value_offset=used; number(2,4);
        size_t length=used;
        assert(xsettings_scale(data,length)==2); /* Do not multiply DPI by window scale. */
        for(size_t n=0;n<length;n++) assert(xsettings_scale(data,n)==0);
        used=value_offset; number(1,4); assert(xsettings_scale(data,length)==1);
        used=value_offset; number(0xffffffff,4); assert(xsettings_scale(data,length)==3);
        begin(1); header(0,"Xft/DPI"); number(144*1024,4);
        assert(xsettings_scale(data,used)==1.5f);
        begin(1); header(0,"Gdk/WindowScalingFactor"); number(0,4);
        assert(xsettings_scale(data,used)==0);
        begin(1); header(1,"Net/ThemeName"); number(0xffffffff,4);
        assert(xsettings_scale(data,used)==0); /* Oversized string must not overrun. */
        begin(1); header(3,"Unknown"); number(2,4);
        assert(xsettings_scale(data,used)==0);
        begin(0xffffffff); assert(xsettings_scale(data,used)==0);
    }
    assert(xsettings_scale(NULL,0)==0);
    begin(0); data[0]=2; assert(xsettings_scale(data,used)==0);
    puts("Indicator scale tests passed: byte orders, DPI fallback, precedence, and malformed settings.");
    return 0;
}
