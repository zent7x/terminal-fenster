#include <stdio.h>
#include <stdint.h>
#include <mach/mach_time.h>
#include <time.h>
int main(void){
  mach_timebase_info_data_t tb; mach_timebase_info(&tb);
  uint64_t abs_ = mach_absolute_time();
  uint64_t con_ = mach_continuous_time();
  uint64_t up   = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  uint64_t mraw = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW);
  uint64_t mon  = clock_gettime_nsec_np(CLOCK_MONOTONIC);
  struct timespec rt; clock_gettime(CLOCK_REALTIME,&rt);
  printf("{\"abs_ns\":%llu,\"con_ns\":%llu,\"UPTIME_RAW\":%llu,\"MONOTONIC_RAW\":%llu,\"MONOTONIC\":%llu,\"REALTIME_ns\":%llu,\"con_minus_abs_ns\":%lld}\n",
    abs_*tb.numer/tb.denom, con_*tb.numer/tb.denom, up, mraw, mon,
    (uint64_t)rt.tv_sec*1000000000ULL+rt.tv_nsec,
    (long long)((con_-abs_)*tb.numer/tb.denom));
  return 0;
}
