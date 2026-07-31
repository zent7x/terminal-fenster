#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <string.h>
static inline uint64_t now(void){return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);}
int main(int argc,char**argv){
  size_t N = argc>1? atoll(argv[1]) : (5184000*4/3+64); // ~b64 of 1440x900 BGRA
  char *buf = malloc(N); memset(buf,'A',N);
  int fd = open("/dev/null", O_WRONLY);
  int iters=200; uint64_t t0=now();
  for(int i=0;i<iters;i++){ ssize_t w=0; while(w<(ssize_t)N){ ssize_t r=write(fd,buf+w,N-w); if(r<=0) break; w+=r;} }
  uint64_t t1=now();
  printf("write() %zu bytes -> /dev/null: %.3f ms/iter, %.2f GB/s\n", N, (t1-t0)/1e6/iters, (double)N*iters/((t1-t0)/1e9)/1e9);
  // pipe target (closer to a pty)
  int p[2]; pipe(p); fcntl(p[0],F_SETFL,O_NONBLOCK);
  char sink[1<<16];
  t0=now(); size_t total=0;
  for(int i=0;i<20;i++){ size_t w=0; while(w<N){ ssize_t r=write(p[1],buf+w,N-w>65536?65536:N-w); if(r>0){w+=r;total+=r;} else { ssize_t d; while((d=read(p[0],sink,sizeof sink))>0); } while(read(p[0],sink,sizeof sink)>0); } }
  t1=now();
  printf("write()->pipe(64K chunks, drained): %.3f ms per %zu bytes, %.2f GB/s\n",(t1-t0)/1e6/20.0,N,(double)total/((t1-t0)/1e9)/1e9);
  return 0;
}
