# Host build + unit tests for the BLUR node firmware core.
# No hardware, no cross toolchain: this proves the protocol codec and the
# node control law / envelope on the host. The G431 and ESP32-S3 node
# projects link node_core.c and protocol.h unchanged.
CC      ?= cc
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Werror -pedantic
LDLIBS  := -lm

.PHONY: test clean
test: test_node_core
	./test_node_core

test_node_core: test_node_core.c node_core.c node_core.h protocol.h
	$(CC) $(CFLAGS) -o $@ test_node_core.c node_core.c $(LDLIBS)

clean:
	rm -f test_node_core
