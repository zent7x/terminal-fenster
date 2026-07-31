#include <stdio.h>
#include <libproc.h>
#include <sys/proc_info.h>
#include <unistd.h>
#include <string.h>
#include <stdlib.h>
#include <mach/mach.h>
int main(int argc, char**argv){
  pid_t p = argc>1? atoi(argv[1]) : getpid();
  struct rusage_info_v6 ri; memset(&ri,0,sizeof ri);
  int rc = proc_pid_rusage(p, RUSAGE_INFO_V6, (rusage_info_t*)&ri);
  printf("proc_pid_rusage rc=%d pid=%d\n", rc, p);
  if(rc==0){
    printf("  ri_user_time=%llu ns\n", ri.ri_user_time);
    printf("  ri_system_time=%llu ns\n", ri.ri_system_time);
    printf("  ri_phys_footprint=%llu bytes\n", ri.ri_phys_footprint);
    printf("  ri_resident_size=%llu bytes\n", ri.ri_resident_size);
    printf("  ri_lifetime_max_phys_footprint=%llu\n", ri.ri_lifetime_max_phys_footprint);
    printf("  ri_pkg_idle_wkups=%llu ri_interrupt_wkups=%llu\n", ri.ri_pkg_idle_wkups, ri.ri_interrupt_wkups);
    printf("  ri_instructions=%llu ri_cycles=%llu\n", ri.ri_instructions, ri.ri_cycles);
    printf("  ri_billed_energy=%llu ri_serviced_energy=%llu\n", ri.ri_billed_energy, ri.ri_serviced_energy);
    printf("  ri_proc_start_abstime=%llu ri_proc_exit_abstime=%llu\n", ri.ri_proc_start_abstime, ri.ri_proc_exit_abstime);
  }
  // TASK_VM_INFO phys_footprint for self
  task_vm_info_data_t vmi; mach_msg_type_number_t cnt = TASK_VM_INFO_COUNT;
  kern_return_t kr = task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&vmi, &cnt);
  printf("task_info TASK_VM_INFO kr=%d phys_footprint=%llu resident=%llu\n", kr, (unsigned long long)vmi.phys_footprint, (unsigned long long)vmi.resident_size);
  return 0;
}
