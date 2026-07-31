# Machine-characterization probes for A10-performance-plan.md §9.5.
# Every number in §0 of that document was produced by these programs.
# No dependencies. `make && make run`.
CC     ?= clang
CFLAGS ?= -O2 -Wall
BINS    = tb clocks ru wr pty3 cct cid

all: $(BINS)
%: %.c
	$(CC) $(CFLAGS) -o $@ $<
run: all
	@for b in $(BINS); do echo "===== $$b ====="; ./$$b; echo; done
clean:
	rm -f $(BINS)
.PHONY: all run clean
