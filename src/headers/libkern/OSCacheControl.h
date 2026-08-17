#ifndef _OSCACHECONTROL_H
#define _OSCACHECONTROL_H
void sys_icache_invalidate(void *, unsigned long);
void sys_dcache_flush(void *, unsigned long);
#endif
