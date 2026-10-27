//! Raw bindings to the C++ simulation kernel plus a safe wrapper.
//!
//! The structs mirror `core/include/fleet/c_api.h` field for field. `assert_abi`
//! compares `size_of` against what the C++ side reports so a layout drift is
//! caught at start-up rather than as silent memory corruption.

use anyhow::{bail, Result};
use std::ffi::CStr;
use std::os::raw::c_char;

pub const MAX_FAULTS: usize = 8;
pub const MAX_HOTSPOTS: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FaultC {
    pub kind: i32,
    pub start_tick: i32,
    pub duration: i32,
    pub cx: i32,
    pub cy: i32,
    pub radius: i32,
    pub intensity: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScenarioC {
    pub seed: u64,
    pub grid: i32,
    pub vehicles: i32,
    pub ticks: i32,
    pub max_wait_ticks: i32,
    pub base_rate: f64,
    pub hotspot_share: f64,
    pub n_hotspots: i32,
    pub n_faults: i32,
    pub hotspot_x: [i32; MAX_HOTSPOTS],
    pub hotspot_y: [i32; MAX_HOTSPOTS],
    pub hotspot_w: [f64; MAX_HOTSPOTS],
    pub faults: [FaultC; MAX_FAULTS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsC {
    pub state_hash: u64,
    pub vehicle_ticks: u64,
    pub requests_total: i64,
    pub completed: i64,
    pub abandoned: i64,
    pub cancelled_phantom: i64,
    pub pending_end: i64,
    pub in_progress_end: i64,
    pub congestion_events: i64,
    pub fault_vehicle_ticks: i64,
    pub invariant_violations: i64,
    pub deadhead_cells: i64,
    pub loaded_cells: i64,
    pub max_occupancy: i32,
    pub wait_p95_ticks: i32,
    pub wait_mean_ticks: f64,
    pub trip_mean_ticks: f64,
    pub utilization: f64,
}

extern "C" {
    fn fleet_run(scenario: *const ScenarioC, out: *mut MetricsC) -> i32;
    fn fleet_scenario_size() -> usize;
    fn fleet_metrics_size() -> usize;
    fn fleet_version() -> *const c_char;
}

/// Verifies that the Rust and C++ struct layouts agree.
pub fn assert_abi() -> Result<()> {
    let (s, m) = unsafe { (fleet_scenario_size(), fleet_metrics_size()) };
    if s != std::mem::size_of::<ScenarioC>() || m != std::mem::size_of::<MetricsC>() {
        bail!(
            "ABI mismatch: scenario {s} vs {}, metrics {m} vs {}",
            std::mem::size_of::<ScenarioC>(),
            std::mem::size_of::<MetricsC>()
        );
    }
    Ok(())
}

pub fn core_version() -> String {
    unsafe { CStr::from_ptr(fleet_version()).to_string_lossy().into_owned() }
}

/// Runs one scenario in the C++ kernel. Safe to call from many threads at once.
pub fn run(scenario: &ScenarioC) -> Result<MetricsC> {
    let mut out = MetricsC::default();
    // SAFETY: both pointers are valid for the duration of the call and the kernel
    // keeps all state local to the call.
    let rc = unsafe { fleet_run(scenario, &mut out) };
    if rc != 0 {
        bail!("fleet_run rejected the scenario (code {rc})");
    }
    Ok(out)
}
