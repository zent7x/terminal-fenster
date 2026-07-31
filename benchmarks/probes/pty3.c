#define _DARWIN_C_SOURCE 1
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <string.h>
#include <termios.h>
#include <pthread.h>
static inline uint64_t now(void){return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);}
static int master_fd; static volatile int stop_=0;
static void* drain(void*a){ char b[1<<16]; while(!stop_){ ssize_t r=read(master_fd,b,sizeof b); if(r<=0) break;} return 0;}
static int cmp(const void*a,const void*b){ uint64_t x=*(uint64_t*)a,y=*(uint64_t*)b; return x<y?-1:x>y;}
int main(void){
  int m=posix_openpt(O_RDWR|O_NOCTTY); grantpt(m); unlockpt(m); master_fd=m;
  int s=open(ptsname(m),O_RDWR|O_NOCTTY);
  struct termios t; tcgetattr(s,&t); cfmakeraw(&t); t.c_oflag&=~OPOST; tcsetattr(s,TCSANOW,&t);
  pthread_t th; pthread_create(&th,0,drain,0);
  size_t sizes[4]={ 6912064 /*1440x900 b64 RGBA*/, 1728000 /*720x450 b64*/, 400000 /*half-block text 200x50*/, 40000 };
  const char* nm[4]={"1440x900 b64-RGBA","720x450 b64-RGBA","half-block text ~400KB","small delta 40KB"};
  size_t chunks[3]={1024,8192,65536};
  char*buf=malloc(8<<20); memset(buf,'A',8<<20);
  for(int z=0;z<4;z++) for(int k=0;k<3;k++){
    size_t N=sizes[z], chunk=chunks[k]; int iters=51; uint64_t sam[51];
    for(int i=0;i<iters;i++){ uint64_t a=now(); size_t w=0;
      while(w<N){ size_t c=(N-w<chunk)?N-w:chunk; ssize_t r=write(s,buf+w,c); if(r>0)w+=r; else break;}
      sam[i]=now()-a; }
    qsort(sam+1,iters-1,sizeof(uint64_t),cmp); // discard first (warmup)
    printf("%-24s N=%7zu chunk=%5zu  p50=%7.3f ms  p95=%7.3f ms  p99=%7.3f ms  (p50 => %6.1f fps)\n",
      nm[z],N,chunk, sam[1+(iters-1)/2]/1e6, sam[1+(int)((iters-1)*0.95)]/1e6, sam[iters-1]/1e6,
      1e9/(double)sam[1+(iters-1)/2]);
  }
  stop_=1; close(s); return 0;
}
