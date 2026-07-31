#include <stdio.h>
#include <mach/mach_time.h>
#include <time.h>
static inline uint64_t n(void){return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);}
int main(void){ volatile uint64_t s=0; const int N=2000000;
 uint64_t a=n(); for(int i=0;i<N;i++) s+=mach_continuous_time(); uint64_t b=n();
 printf("mach_continuous_time = %.2f ns/call\n",(double)(b-a)/N);
 a=n(); for(int i=0;i<N;i++) s+=mach_absolute_time(); b=n();
 printf("mach_absolute_time   = %.2f ns/call\n",(double)(b-a)/N);
 mach_timebase_info_data_t t; mach_timebase_info(&t);
 a=n(); for(int i=0;i<N;i++) s+=mach_continuous_time()*t.numer/t.denom; b=n();
 printf("continuous*125/3     = %.2f ns/call\n",(double)(b-a)/N);
 return 0;}
