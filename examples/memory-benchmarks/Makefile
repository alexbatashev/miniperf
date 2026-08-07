CC ?= gcc
BUILD_DIR ?= ../../target/memory-benchmarks/bin
COLLECTOR_DIR ?= ../../target/debug/deps

COMMON_FLAGS := -O3 -g -std=c17 -Wall -Wextra -Wpedantic -march=x86-64 -mavx2 -mfma
BENCHMARKS := matmul stream pointer-chase stencil
ROOFLINE_BENCHMARKS := matmul-roofline stream-roofline stencil-roofline

.PHONY: all roofline clean

all: $(BENCHMARKS:%=$(BUILD_DIR)/%)

roofline: $(ROOFLINE_BENCHMARKS:%=$(BUILD_DIR)/%)

$(BUILD_DIR):
	mkdir -p $@

$(BUILD_DIR)/%: %.c bench.h | $(BUILD_DIR)
	$(CC) $(COMMON_FLAGS) $< -o $@

$(BUILD_DIR)/%-roofline: %.c bench.h roofline.h | $(BUILD_DIR)
	$(CC) $(COMMON_FLAGS) -DMINIPERF_MANUAL_ROOFLINE $< \
		-L$(COLLECTOR_DIR) -lcollector \
		-Wl,-rpath,'$$ORIGIN/../../debug/deps' -o $@

clean:
	rm -rf $(BUILD_DIR)
