// Compiles the C++ simulation kernel and links it into the Rust binary.
fn main() {
    let core = "../core";
    println!("cargo:rerun-if-changed={core}/src/sim.cpp");
    println!("cargo:rerun-if-changed={core}/include/fleet/c_api.h");
    println!("cargo:rerun-if-changed={core}/include/fleet/rng.hpp");

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file(format!("{core}/src/sim.cpp"))
        .include(format!("{core}/include"))
        .opt_level(3)
        // No fused multiply-add: keeps floating point results identical across builds.
        .flag_if_supported("-ffp-contract=off")
        .warnings(true)
        .compile("fleetcore");
}
