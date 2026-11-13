//! The flags of the release build (plan section 12, release condition 2): the RUSTFLAGS that
//! `.github/scripts/release_build.sh` gives cargo remap the checkout and CARGO_HOME on every
//! system, and on Windows also pass `/Brepro` (spelled `-Brepro`) to link.exe, without which
//! `unio.exe` carries the link time and two release builds differ. The script prints the flags
//! with `--print-rustflags SYSTEM` and builds nothing, so this runs on any Unix machine; on the
//! Windows runner the job `release` checks the flags of the real system (step "The release
//! flags pass /Brepro to link.exe").

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

fn flags(system: &str) -> String {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/release_build.sh");
    let output = Command::new("sh")
        .arg(&script)
        .args(["--print-rustflags", system])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn windows_release_links_with_brepro() {
    for system in [
        "MINGW64_NT-10.0-26100",
        "MSYS_NT-10.0-26100",
        "CYGWIN_NT-10.0",
    ] {
        let flags = flags(system);
        let words: Vec<&str> = flags.split_whitespace().collect();
        let link_args: Vec<&str> = words
            .windows(2)
            .filter(|pair| pair[0] == "-C")
            .map(|pair| pair[1])
            .collect();
        assert!(link_args.contains(&"link-arg=-Brepro"), "{system}: {flags}");
        assert!(
            link_args.contains(&"link-arg=-PDBALTPATH:%_PDB%"),
            "{system}: {flags}"
        );
        assert!(flags.contains("=/unio "), "{system}: {flags}");
        assert!(flags.contains("=/cargo "), "{system}: {flags}");
    }
}

#[test]
fn other_systems_keep_the_remapping_alone() {
    for system in ["Linux", "Darwin"] {
        let flags = flags(system);
        assert!(!flags.contains("Brepro"), "{system}: {flags}");
        assert_eq!(
            flags.matches("--remap-path-prefix=").count(),
            2,
            "{system}: {flags}"
        );
        assert!(flags.trim_end().ends_with("=/cargo"), "{system}: {flags}");
    }
}
