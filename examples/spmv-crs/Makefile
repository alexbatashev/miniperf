CC ?= gcc
RISCV_CC ?= riscv64-linux-gnu-gcc
BUILD_DIR ?= build

COMMON_FLAGS := -O3 -g -std=c17 -Wall -Wextra -Wpedantic -fopenmp

.PHONY: all clean

all: $(BUILD_DIR)/spmv-avx512 $(BUILD_DIR)/spmv-rvv

$(BUILD_DIR):
	mkdir -p $@

$(BUILD_DIR)/spmv-avx512: spmv_crs.c | $(BUILD_DIR)
	$(CC) $(COMMON_FLAGS) -mavx512f -mavx512dq -mavx512vl -mfma $< -lm -o $@

$(BUILD_DIR)/spmv-rvv: spmv_crs.c | $(BUILD_DIR)
	$(RISCV_CC) $(COMMON_FLAGS) -march=rv64gcv -mabi=lp64d $< -lm -o $@

clean:
	rm -rf $(BUILD_DIR)
