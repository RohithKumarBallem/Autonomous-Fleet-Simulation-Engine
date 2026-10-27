CXX      ?= c++
CXXFLAGS ?= -std=c++17 -O3 -Wall -Wextra -ffp-contract=off -Iinclude
BUILD    := build

all: $(BUILD)/fleetsim $(BUILD)/test_core

$(BUILD)/sim.o: src/sim.cpp include/fleet/c_api.h include/fleet/rng.hpp | $(BUILD)
	$(CXX) $(CXXFLAGS) -c $< -o $@

$(BUILD)/fleetsim: src/main.cpp $(BUILD)/sim.o
	$(CXX) $(CXXFLAGS) $^ -o $@

$(BUILD)/test_core: tests/test_core.cpp $(BUILD)/sim.o
	$(CXX) $(CXXFLAGS) $^ -o $@

$(BUILD):
	mkdir -p $(BUILD)

test: $(BUILD)/test_core
	./$(BUILD)/test_core

clean:
	rm -rf $(BUILD)

.PHONY: all test clean
