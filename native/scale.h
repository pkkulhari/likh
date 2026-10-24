/* XSETTINGS uses the manager's byte order, not necessarily the client's. */
#ifndef LIKH_SCALE_H
#define LIKH_SCALE_H
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static float valid_scale(double value) {
    return value>=1.0 && value<=4.0 ? (float)value : 0.0f;
}
static float scale_number(const char *text,double divisor) {
    if(!text || !*text) return 0;
    char *end;
    double value=strtod(text,&end);
    while(*end==' ' || *end=='\t' || *end=='\n') end++;
    return end!=text && !*end ? valid_scale(value/divisor) : 0;
}
static uint32_t settings_uint(const unsigned char *p,int bytes,int big_endian) {
    uint32_t value=0;
    for(int i=0;i<bytes;i++) value=(value<<8)|p[big_endian?i:bytes-1-i];
    return value;
}
static float xsettings_scale(const unsigned char *data,size_t length) {
    if(!data || length<12 || data[0]>1) return 0;
    int big_endian=data[0];
    uint32_t count=settings_uint(data+8,4,big_endian);
    size_t offset=12;
    float window_scale=0,dpi_scale=0;
    for(uint32_t i=0;i<count;i++) {
        if(length-offset<4) return 0;
        int type=data[offset];
        size_t name_length=settings_uint(data+offset+2,2,big_endian);
        offset+=4;
        size_t padded=(name_length+3)&~(size_t)3;
        if(length-offset<padded+4) return 0;
        const unsigned char *name=data+offset;
        offset+=padded+4; /* name, padding, last-change serial */
        if(type==0) {
            if(length-offset<4) return 0;
            uint32_t value=settings_uint(data+offset,4,big_endian);
            if(name_length==sizeof("Gdk/WindowScalingFactor")-1 &&
               !memcmp(name,"Gdk/WindowScalingFactor",name_length)) window_scale=valid_scale(value);
            if(name_length==sizeof("Xft/DPI")-1 && !memcmp(name,"Xft/DPI",name_length))
                dpi_scale=valid_scale((double)value/(96*1024));
            offset+=4;
        } else if(type==1) {
            if(length-offset<4) return 0;
            uint32_t size=settings_uint(data+offset,4,big_endian);
            offset+=4;
            if(size>length-offset) return 0;
            offset+=size;
            size_t padding=(4-size%4)%4;
            if(padding>length-offset) return 0;
            offset+=padding;
        } else if(type==2) {
            if(length-offset<8) return 0;
            offset+=8;
        } else return 0;
    }
    /* DPI can include a separate text-size preference; never multiply scales. */
    return window_scale?window_scale:dpi_scale;
}
#endif
