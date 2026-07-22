//! Embeds the compile-time target triple so `vaire upgrade` can pick the matching
//! release asset (the release workflow names assets `vaire-<tag>-<triple>`).

fn main() {
    println!(
        "cargo:rustc-env=VAIRE_TARGET_TRIPLE={}",
        std::env::var("TARGET").expect("cargo sets TARGET")
    );
}
