//! The identity of the toolchain that links an executable (docs/runtime.md 8.4, C-4). Pinning
//! the code that finds the linker does not pin the linker it finds, so every build records the
//! linker and the SDK it was made with, in the program image ([`super::image`]): their tool names
//! and versions, never a path of the machine, so that the record is the same wherever the same
//! toolchain is installed.
//!
//! - Linux: the C compiler (`cc`, or `CC`) and the first line of its `--version`;
//! - macOS: `clang` of the selected developer tools (or `CC`), the first line of its
//!   `--version`, and the version of the SDK (`xcrun --show-sdk-version`, or the name of the SDK);
//! - Windows: the version of the MSVC toolset of `link.exe` (from its directory
//!   `VC\Tools\MSVC\<version>`, or from its banner) and that of the Windows SDK (from the
//!   directory `Windows Kits\10\lib\<version>\um` in `LIB`).
//!
//! The tool computes it before the compile step and hands it to the worker; `unio build
//! --print-toolchain` prints it. A missing linker stops here with a message that says what to
//! install; nothing is installed.

use std::process::{Command, Stdio};

use super::Target;

/// The identity of the toolchain that `unio build` links with on this machine.
pub fn identity() -> Result<String, String> {
    let Some(target) = Target::host() else {
        return Err(super::unsupported());
    };
    identity_for(target)
}

/// The identity of the toolchain that `unio build --target` links with for `target` on this
/// machine: for the other Linux target, the cross C compiler of that target
/// ([`super::linux_compiler_for`]). A pair of machines that cannot be built for stops here
/// ([`super::cross_problem`]).
pub fn identity_for(target: Target) -> Result<String, String> {
    super::cross_problem(target).map_or(Ok(()), Err)?;
    match target {
        Target::LinuxX86_64 | Target::LinuxAarch64 if Some(target) != Target::host() => {
            let cc = super::linux_compiler_for(target);
            let banner = first_line(&cc, &["--version"]).map_err(|e| {
                format!(
                    "the C compiler {cc} that links executables for {} cannot be run ({e}); install a cross C compiler for {} with its linker and C library (on Debian and Ubuntu the package {}), or name one in {}; unio does not install it",
                    target.name(),
                    target.rust_triple(),
                    super::cross_package(target),
                    super::cross_compiler_variable(target)
                )
            })?;
            Ok(linux(target, &cc, &banner))
        }
        Target::LinuxX86_64 | Target::LinuxAarch64 => {
            let cc = super::linux_compiler();
            let banner = first_line(&cc, &["--version"]).map_err(|e| {
                format!(
                    "the C compiler {cc} that links executables cannot be run ({e}); install a C compiler with its linker (gcc or clang with binutils, such as the package build-essential), or name one in CC; unio does not install it"
                )
            })?;
            Ok(linux(target, &cc, &banner))
        }
        Target::MacosAarch64 => {
            let clang = super::macos_compiler()?;
            let banner = first_line(&clang, &["--version"]).map_err(|e| {
                format!(
                    "the compiler {clang} that links executables cannot be run ({e}); install the Xcode command line tools with xcode-select --install; unio does not install them"
                )
            })?;
            let sdk = super::xcrun(&["--show-sdk-path"])?;
            let version = super::xcrun(&["--show-sdk-version"]).ok();
            Ok(macos(target, &clang, &banner, version.as_deref(), &sdk))
        }
        Target::WindowsX86_64 => windows_identity(target),
    }
}

#[cfg(windows)]
fn windows_identity(target: Target) -> Result<String, String> {
    let tool = super::find_msvc().ok_or(super::MSVC_MISSING)?;
    let lib = tool
        .env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("LIB"))
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var_os("LIB"))
        .map(|v| v.to_string_lossy().into_owned());
    let path = tool.path.to_string_lossy().into_owned();
    // The banner only when the directory does not name the version (a copy of link.exe).
    let banner = if msvc_version(&path).is_none() {
        Command::new(&tool.path)
            .envs(tool.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        None
    };
    Ok(windows(target, &path, lib.as_deref(), banner.as_deref()))
}

#[cfg(not(windows))]
fn windows_identity(_: Target) -> Result<String, String> {
    Err(super::unsupported())
}

/// The first line of what `program args` prints, in the C locale.
fn first_line(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(line) if output.status.success() => Ok(line.to_string()),
        _ => Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// The name of a program without its directory, on any system.
pub fn tool_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

/// The identity on Linux: the target, the C compiler, and the first line of its `--version`.
pub fn linux(target: Target, cc: &str, banner: &str) -> String {
    format!("{}; {}: {}", target.triple(), tool_name(cc), banner.trim())
}

/// The version of a macOS SDK from its path, as in `.../SDKs/MacOSX15.2.sdk`; `None` for the
/// unversioned link `MacOSX.sdk`.
pub fn macos_sdk_version(sdk_path: &str) -> Option<String> {
    let name = tool_name(sdk_path.trim_end_matches(['/', '\\']));
    let version = name.strip_prefix("MacOSX")?.strip_suffix(".sdk")?;
    (!version.is_empty()).then(|| version.to_string())
}

/// The identity on macOS: the target, the compiler, the first line of its `--version`, and the
/// version of the SDK (`xcrun --show-sdk-version`, else from the SDK's name).
pub fn macos(
    target: Target,
    clang: &str,
    banner: &str,
    sdk_version: Option<&str>,
    sdk_path: &str,
) -> String {
    let sdk = sdk_version
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| macos_sdk_version(sdk_path))
        .unwrap_or_else(|| "unknown".into());
    format!(
        "{}; {}: {}; sdk: macOS {sdk}",
        target.triple(),
        tool_name(clang),
        banner.trim()
    )
}

/// The components of a Windows path, split on either separator.
fn components(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).filter(|c| !c.is_empty()).collect()
}

/// Whether `s` is a version: digits separated by dots.
fn is_version(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The version of the MSVC toolset of a `link.exe` from its path,
/// `...\VC\Tools\MSVC\<version>\bin\Hostx64\x64\link.exe`.
pub fn msvc_version(link_path: &str) -> Option<String> {
    let parts = components(link_path);
    parts.windows(4).find_map(|w| {
        (w[0].eq_ignore_ascii_case("VC")
            && w[1].eq_ignore_ascii_case("Tools")
            && w[2].eq_ignore_ascii_case("MSVC")
            && is_version(w[3]))
        .then(|| w[3].to_string())
    })
}

/// The version in the banner of `link.exe`: "Microsoft (R) Incremental Linker Version 14.51.36231.0".
pub fn linker_banner_version(banner: &str) -> Option<String> {
    banner.lines().find_map(|line| {
        let (_, after) = line.split_once("Linker Version ")?;
        let version = after.split_whitespace().next()?;
        is_version(version).then(|| version.to_string())
    })
}

/// The version of the Windows SDK from `LIB`: the entry `...\Windows Kits\10\lib\<version>\um\x64`.
pub fn windows_sdk_version(lib: &str) -> Option<String> {
    lib.split(';').find_map(|entry| {
        let parts = components(entry);
        parts.windows(5).find_map(|w| {
            (w[0].eq_ignore_ascii_case("Windows Kits")
                && is_version(w[1])
                && w[2].eq_ignore_ascii_case("lib")
                && is_version(w[3])
                && w[4].eq_ignore_ascii_case("um"))
            .then(|| w[3].to_string())
        })
    })
}

/// The identity on Windows: the target, the version of the MSVC toolset of `link.exe` (from its
/// path, else from its banner), and the version of the Windows SDK in `LIB`.
pub fn windows(target: Target, link_path: &str, lib: Option<&str>, banner: Option<&str>) -> String {
    let msvc = msvc_version(link_path)
        .or_else(|| banner.and_then(linker_banner_version))
        .unwrap_or_else(|| "unknown".into());
    let sdk = lib
        .and_then(windows_sdk_version)
        .unwrap_or_else(|| "unknown".into());
    format!(
        "{}; {}: MSVC {msvc}; sdk: Windows {sdk}",
        target.triple(),
        tool_name(link_path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = r"C:\Program Files\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\link.exe";
    const LIB: &str = r"C:\Program Files\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\lib\x64;C:\Program Files (x86)\Windows Kits\10\lib\10.0.26100.0\ucrt\x64;C:\Program Files (x86)\Windows Kits\10\lib\10.0.26100.0\um\x64";

    #[test]
    fn windows_versions_come_from_the_paths_of_the_linker_and_the_sdk() {
        assert_eq!(msvc_version(LINK).as_deref(), Some("14.51.36231"));
        assert_eq!(windows_sdk_version(LIB).as_deref(), Some("10.0.26100.0"));
        // Other spellings of the same directories.
        assert_eq!(
            msvc_version(r"D:/vs/vc/tools/msvc/14.44.35207/bin/HostX64/x64/link.exe").as_deref(),
            Some("14.44.35207")
        );
        assert_eq!(
            windows_sdk_version(r"x;D:\Kits\Windows Kits\10\Lib\10.0.22621.0\um\x64\;").as_deref(),
            Some("10.0.22621.0")
        );
        assert_eq!(msvc_version(r"C:\tools\link.exe"), None);
        assert_eq!(windows_sdk_version(r"C:\libs;C:\other"), None);
        assert_eq!(
            linker_banner_version(
                "Microsoft (R) Incremental Linker Version 14.51.36231.0\r\nCopyright (C) Microsoft Corporation.  All rights reserved.\r\n"
            )
            .as_deref(),
            Some("14.51.36231.0")
        );
        assert_eq!(linker_banner_version("usage: link [options]"), None);
    }

    #[test]
    fn the_windows_identity_names_versions_and_no_path() {
        let id = windows(Target::WindowsX86_64, LINK, Some(LIB), None);
        assert_eq!(
            id,
            "x86_64-pc-windows-msvc; link.exe: MSVC 14.51.36231; sdk: Windows 10.0.26100.0"
        );
        // A link.exe outside a toolset directory: the version of its banner.
        let id = windows(
            Target::WindowsX86_64,
            r"C:\Users\someone\bin\link.exe",
            None,
            Some("Microsoft (R) Incremental Linker Version 14.44.35207.1\n"),
        );
        assert_eq!(
            id,
            "x86_64-pc-windows-msvc; link.exe: MSVC 14.44.35207.1; sdk: Windows unknown"
        );
        assert!(!id.contains("someone"));
    }

    #[test]
    fn the_macos_identity_names_the_compiler_and_the_sdk() {
        let banner = "Apple clang version 17.0.0 (clang-1700.0.13.5)";
        let clang = "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang";
        let sdk = "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX15.5.sdk";
        assert_eq!(
            macos(Target::MacosAarch64, clang, banner, Some("15.5\n"), sdk),
            "aarch64-apple-macosx11.0.0; clang: Apple clang version 17.0.0 (clang-1700.0.13.5); sdk: macOS 15.5"
        );
        // Without the version from xcrun, the name of the SDK gives it.
        assert_eq!(
            macos(Target::MacosAarch64, clang, banner, None, sdk),
            macos(Target::MacosAarch64, clang, banner, Some("15.5"), sdk)
        );
        assert_eq!(macos_sdk_version(sdk).as_deref(), Some("15.5"));
        assert_eq!(
            macos_sdk_version("/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/"),
            None
        );
        assert!(macos(
            Target::MacosAarch64,
            "clang",
            banner,
            None,
            "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk"
        )
        .ends_with("; sdk: macOS unknown"));
    }

    #[test]
    fn the_linux_identity_names_the_compiler_without_its_directory() {
        assert_eq!(
            linux(
                Target::LinuxX86_64,
                "/home/someone/toolchains/bin/gcc-14",
                "gcc-14 (Ubuntu 14.2.0-4ubuntu2) 14.2.0\n"
            ),
            "x86_64-unknown-linux-gnu; gcc-14: gcc-14 (Ubuntu 14.2.0-4ubuntu2) 14.2.0"
        );
        assert_eq!(
            linux(Target::LinuxAarch64, "cc", "clang version 19.1.7"),
            "aarch64-unknown-linux-gnu; cc: clang version 19.1.7"
        );
    }

    /// On this machine, where a linker is installed, the identity is the same each time.
    #[test]
    fn the_identity_of_this_machine_is_stable() {
        if Target::host().is_none() || cfg!(windows) {
            // Windows: the tool sets the finder of link.exe; tests/native.rs covers it there.
            return;
        }
        let Ok(first) = identity() else {
            eprintln!("skipped: no linker on this machine");
            return;
        };
        assert_eq!(identity().unwrap(), first);
        assert!(
            first.starts_with(Target::host().unwrap().triple()),
            "{first}"
        );
        assert!(!first.contains('\n'), "{first}");
    }
}
