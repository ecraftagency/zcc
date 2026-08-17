#ifndef _SYS_TIME_H
#define _SYS_TIME_H
/* layout Darwin: tv_sec long, tv_usec int */
struct timeval {
    long tv_sec;
    int tv_usec;
};
struct timezone {
    int tz_minuteswest;
    int tz_dsttime;
};
int gettimeofday(struct timeval *, struct timezone *);
#endif
