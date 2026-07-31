#include <stdio.h>
#include <mach/mach_time.h>
#include <time.h>
#include <sys/resource.h>
#include <unistd.h>
int main(void){
  mach_timebase_info_data_t tb; mach_timebase_info(&tb);
  printf("mach_timebase numer=%u denom=%u\n", tb.numer, tb.denom);
  uint64_t a = mach_absolute_time();
  uint64_t b = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  uint64_t c = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW);
  printf("mach_absolute_time=%llu\nCLOCK_UPTIME_RAW=%llu\nCLOCK_MONOTONIC_RAW=%llu\n", a,b,c);
  // resolution probe: min nonzero delta over 100k reads
  uint64_t mind = ~0ULL; uint64_t prev = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  for(int i=0;i<200000;i++){ uint64_t n=clock_gettime_nsec_np(CLOCK_UPTIME_RAW); if(n>prev && n-prev<mind) mind=n-prev; prev=n; }
  printf("min nonzero UPTIME_RAW delta = %llu ns\n", mind);
  // cost per call
  uint64_t t0=clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  volatile uint64_t s=0; for(int i=0;i<1000000;i++) s+=clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  uint64_t t1=clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  printf("clock_gettime_nsec_np cost = %.2f ns/call\n", (double)(t1-t0)/1000000.0);
  t0=clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  for(int i=0;i<1000000;i++) s+=mach_absolute_time();
  t1=clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  printf("mach_absolute_time cost = %.2f ns/call\n", (double)(t1-t0)/1000000.0);
  struct rusage ru; getrusage(RUSAGE_SELF,&ru);
  printf("maxrss=%ld (bytes on macOS) utime=%ld.%06d stime=%ld.%06d nvcsw=%ld nivcsw=%ld\n",
    ru.ru_maxrss, (long)ru.ru_utime.tv_sec,(int)ru.ru_utime.tv_usec,(long)ru.ru_stime.tv_sec,(int)ru.ru_stime.tv_usec, ru.ru_nvcsw, ru.ru_nivcsw);
  return 0;
}
