#!/usr/bin/env python3
"""The pinned linkers and SDKs of the builds and tests of CI (plan section 12, release
condition 3; compiler/README.md, "Pinned linkers and SDKs").

Rust and the crates are pinned by compiler/rust-toolchain.toml and Cargo.lock. The C compiler,
linker, and SDK that `unio build` links with come from the runner image, so CI selects them
here by exact name and version, and checks that the identity `unio build --print-toolchain`
records in every executable (C-4, R-6) is the one written below.

Usage, from anywhere, with Python 3.9 or newer and the standard library only:

    expected_toolchain.py select       select the pinned toolchain on this runner; with
                                       GITHUB_ENV set, later steps of the job inherit it
    expected_toolchain.py check UNIO   compare `UNIO build --print-toolchain` and the other
                                       pinned components with the pins; exit 1 on a mismatch
    expected_toolchain.py show         print the pins of every platform

An expected record may hold `*` where a part has not yet been read from a CI run: it then
matches any text there, and the check passes with a warning that prints the actual record to
write in its place. The condition is met when no record holds `*` and the check passes.

Updating a pin is a deliberate change, made in one commit: choose the new image, compiler,
Xcode, MSVC toolset, or SDK; change PINS below and the runner labels in
.github/workflows/ci.yml; run CI; copy the actual records the failing check prints into PINS;
and say in the commit why the toolchain changed, since every executable built after it records
another identity.
"""

import os
import platform
import re
import shutil
import subprocess
import sys

# Pins per platform, keyed by the runner they are made for. `record` is the exact line of
# `unio build --print-toolchain`; the other keys say what `select` chooses and what `check`
# compares beside the record (the parts the record does not name).
PINS = {
    "linux-x86_64": {
        "runner": "ubuntu-24.04",
        "cc": "gcc-13",
        # Debian package versions: the compiler, the linker (ld of binutils), and the C
        # library whose start files and static parts are linked into every executable.
        "packages": {
            "gcc-13": "13.3.0-6ubuntu2~24.04.1",
            "binutils": "2.42-4ubuntu2.10",
            "libc6-dev": "2.39-0ubuntu8.9",
        },
        "record": "x86_64-unknown-linux-gnu; gcc-13: gcc-13 (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0",
    },
    "linux-aarch64": {
        "runner": "ubuntu-24.04-arm",
        "cc": "gcc-13",
        "packages": {
            "gcc-13": "13.3.0-6ubuntu2~24.04.1",
            "binutils": "2.42-4ubuntu2.10",
            "libc6-dev": "2.39-0ubuntu8.9",
        },
        "record": "aarch64-unknown-linux-gnu; gcc-13: gcc-13 (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0",
    },
    "macos-arm64": {
        "runner": "macos-26",
        # Xcode 26.6 (build 17F113), the default of the image, with the macOS 26.5 SDK.
        "xcode": "/Applications/Xcode_26.6.app",
        "xcode_version": "Xcode 26.6; Build version 17F113",
        "sdk": "26.5",
        # Read from CI run 43285316778 (image macos-26-arm64 20261102.0473.1).
        "record": "aarch64-apple-macosx11.0.0; clang: Apple clang version 21.0.0 (clang-2100.1.1.101); sdk: macOS 26.5",
    },
    "windows-x86_64": {
        "runner": "windows-2025-vs2026",
        # The MSVC toolset 14.44 of Visual Studio 2022 17.14, installed beside the newer ones of
        # Visual Studio 2026 on the image as a fixed component, and the Windows SDK 10.0.26100.0.
        "msvc": "14.44",
        "sdk": "10.0.26100.0",
        # Read from CI run 43285316778 (image win25-vs2026 20261103.298.1).
        "record": "x86_64-pc-windows-msvc; link.exe: MSVC 14.44.35207; sdk: Windows 10.0.26100.0",
    },
}

HOW_TO_UPDATE = (
    "If the image changed under the pin, update the pin deliberately: choose the toolchain, edit "
    "PINS in .github/scripts/expected_toolchain.py (and the runner labels in "
    ".github/workflows/ci.yml when the image changes), and write the actual record above in "
    "place of the expected one, in a commit that says why (compiler/README.md, "
    '"Pinned linkers and SDKs").'
)


class Mismatch(Exception):
    pass


def platform_key():
    system = platform.system()
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "x64": "x86_64", "arm64": "aarch64"}.get(machine, machine)
    if system == "Linux":
        key = "linux-" + arch
    elif system == "Darwin":
        key = "macos-arm64" if arch == "aarch64" else "macos-" + arch
    elif system == "Windows" or system.startswith(("MINGW", "MSYS", "CYGWIN")):
        key = "windows-" + arch
    else:
        key = system.lower() + "-" + arch
    if key not in PINS:
        raise Mismatch(
            "no toolchain is pinned for {} ({} {}); pinned: {}".format(
                key, system, machine, ", ".join(sorted(PINS))
            )
        )
    return key


def run(args, env=None):
    """The output of a command, or a Mismatch that names it."""
    try:
        done = subprocess.run(
            args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, check=False
        )
    except OSError as e:
        raise Mismatch("{} cannot be run: {}".format(args[0], e))
    if done.returncode != 0:
        raise Mismatch(
            "{} failed with {}: {}".format(
                " ".join(args), done.returncode, done.stderr.decode("utf-8", "replace").strip()
            )
        )
    return done.stdout.decode("utf-8", "replace").replace("\r\n", "\n").strip()


def matches(expected, actual):
    """Whether `actual` is `expected`, where `*` in `expected` stands for any text."""
    pattern = ".*".join(re.escape(part) for part in expected.split("*"))
    return re.fullmatch(pattern, actual) is not None


def export(variables):
    """Writes the variables for the later steps of the job (GITHUB_ENV) and prints them."""
    target = os.environ.get("GITHUB_ENV")
    for name, value in variables.items():
        print("{}={}".format(name, value))
        os.environ[name] = value
    if target:
        with open(target, "a", encoding="utf-8") as f:
            for name, value in variables.items():
                f.write("{}={}\n".format(name, value))


# Selection ------------------------------------------------------------------------------------


def select_linux(key, pin):
    cc = pin["cc"]
    if shutil.which(cc) is None:
        raise Mismatch("{} is not on PATH of this {} runner".format(cc, pin["runner"]))
    triple = "X86_64" if key == "linux-x86_64" else "AARCH64"
    # CC for `unio build` and the tests; the linker of rustc for unio and libunio.a themselves.
    export({"CC": cc, "CARGO_TARGET_{}_UNKNOWN_LINUX_GNU_LINKER".format(triple): cc})


def select_macos(pin):
    developer = pin["xcode"] + "/Contents/Developer"
    if not os.path.isdir(developer):
        raise Mismatch("{} is not installed on this {} runner".format(pin["xcode"], pin["runner"]))
    run(["sudo", "-n", "xcode-select", "-s", developer])
    # DEVELOPER_DIR as well, so that xcrun keeps this Xcode whatever is selected later.
    export({"DEVELOPER_DIR": developer})
    sdk = run(["xcrun", "--show-sdk-version"])
    if sdk != pin["sdk"]:
        raise Mismatch("{} brings the macOS SDK {}, pinned: {}".format(pin["xcode"], sdk, pin["sdk"]))


def windows_kits_root():
    try:
        import winreg

        key = winreg.OpenKey(
            winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows Kits\Installed Roots"
        )
        return winreg.QueryValueEx(key, "KitsRoot10")[0]
    except OSError:
        base = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
        return os.path.join(base, "Windows Kits", "10") + "\\"


def select_windows(pin):
    # In a developer command prompt find-msvc-tools takes link.exe from PATH, where Git Bash
    # puts /usr/bin/link.exe of coreutils first; the pin goes through VCToolsVersion instead.
    for name in ("VCINSTALLDIR", "VSTEL_MSBuildProjectFullPath"):
        if os.environ.get(name):
            raise Mismatch(
                "{} is set: the developer environment would choose link.exe from PATH, not the pin".format(name)
            )
    base = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
    vswhere = os.path.join(base, "Microsoft Visual Studio", "Installer", "vswhere.exe")
    installs = run([vswhere, "-all", "-products", "*", "-property", "installationPath"]).splitlines()
    found = set()
    for install in filter(None, (line.strip() for line in installs)):
        tools = os.path.join(install, "VC", "Tools", "MSVC")
        if not os.path.isdir(tools):
            continue
        for version in os.listdir(tools):
            link = os.path.join(tools, version, "bin", "Hostx64", "x64", "link.exe")
            if version.startswith(pin["msvc"] + ".") and os.path.isfile(link):
                found.add(version)
    if len(found) != 1:
        raise Mismatch(
            "the MSVC toolset {} is installed {} times on this runner ({}); pinned: one".format(
                pin["msvc"], len(found), ", ".join(sorted(found)) or "none"
            )
        )
    toolset = found.pop()
    kits = windows_kits_root()
    kernel32 = os.path.join(kits, "lib", pin["sdk"], "um", "x64", "kernel32.lib")
    if not os.path.isfile(kernel32):
        raise Mismatch("the Windows SDK {} is not installed ({} is missing)".format(pin["sdk"], kernel32))
    # find-msvc-tools takes the Universal CRT from the newest SDK that has one, not from
    # WindowsSDKVersion: it must be the pinned SDK too.
    lib = os.path.join(kits, "lib")
    ucrt = max(
        (v for v in os.listdir(lib) if v.startswith("10.") and os.path.isdir(os.path.join(lib, v, "ucrt"))),
        default=None,
    )
    if ucrt != pin["sdk"]:
        raise Mismatch("the newest Universal CRT is that of the SDK {}, pinned: {}".format(ucrt, pin["sdk"]))
    # find-msvc-tools (and the linker search of rustc) restrict the search to these.
    export({"VCToolsVersion": toolset, "WindowsSdkDir": kits, "WindowsSDKVersion": pin["sdk"] + "\\"})


def select():
    key = platform_key()
    pin = PINS[key]
    print("pinned toolchain of {} (runner {}):".format(key, pin["runner"]))
    if key.startswith("linux-"):
        select_linux(key, pin)
    elif key.startswith("macos-"):
        select_macos(pin)
    else:
        select_windows(pin)


# Check ----------------------------------------------------------------------------------------


def components(key, pin):
    """The pinned components that the record does not name, as (name, expected, actual)."""
    found = []
    if key.startswith("linux-"):
        for package, version in sorted(pin["packages"].items()):
            actual = run(["dpkg-query", "-W", "-f=${Version}", package])
            found.append(("package " + package, version, actual))
        found.append(("CC", pin["cc"], os.environ.get("CC", "(unset)")))
    elif key.startswith("macos-"):
        actual = "; ".join(run(["xcodebuild", "-version"]).splitlines())
        found.append(("xcodebuild -version", pin["xcode_version"], actual))
    else:
        found.append(("VCToolsVersion", pin["msvc"] + ".*", os.environ.get("VCToolsVersion", "(unset)")))
    return found


def check(unio):
    key = platform_key()
    pin = PINS[key]
    image = "{} {}".format(os.environ.get("ImageOS", "?"), os.environ.get("ImageVersion", "?"))
    print("platform {}, runner {}, image {}".format(key, pin["runner"], image))
    actual = run([unio, "build", "--print-toolchain"])
    print("record:   {}".format(actual))
    print("expected: {}".format(pin["record"]))
    failures = []
    if not matches(pin["record"], actual):
        failures.append("the record of the linker and the SDK is not the pinned one")
    for name, expected, value in components(key, pin):
        print("{}: {} (expected {})".format(name, value, expected))
        if not matches(expected, value):
            failures.append("{} is {}, pinned: {}".format(name, value, expected))
    if failures:
        for failure in failures:
            print("::error::{}".format(failure))
        print(HOW_TO_UPDATE)
        return 1
    if "*" in pin["record"]:
        print(
            "::warning::the pin of {} is provisional: write the record {!r} into PINS of "
            ".github/scripts/expected_toolchain.py (release condition 3 is met when no record "
            "holds *)".format(key, actual)
        )
    else:
        print("the linker and the SDK are the pinned ones")
    return 0


def show():
    for key in sorted(PINS):
        pin = PINS[key]
        print("{} ({}): {}".format(key, pin["runner"], pin["record"]))
    return 0


def main(argv):
    try:
        if argv == ["select"]:
            select()
            return 0
        if len(argv) == 2 and argv[0] == "check":
            return check(argv[1])
        if argv == ["show"]:
            return show()
    except Mismatch as e:
        print("::error::{}".format(e))
        print(HOW_TO_UPDATE)
        return 1
    print("usage: expected_toolchain.py select | check UNIO | show", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
