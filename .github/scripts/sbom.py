#!/usr/bin/env python3
"""Writes the software bill of materials (SBOM) of the `unio` release (R-6): a CycloneDX 1.5
JSON document of what the `unio` binary and its runtime library `libunio.a` (`unio.lib`)
contain.

The components are the crates of compiler/Cargo.lock that THIRD-PARTY-NOTICES lists (the
same selection, from `cargo metadata --locked`: normal dependencies for any target, without
build dependencies and procedural macros), each with its version, licence expression, package
URL, the SHA-256 checksum Cargo.lock records for its download, its upstream repository, and the
targets it is compiled for; the Rust standard library of the pinned toolchain; and the
dependency graph between them. The document has no time stamp and its serial number is derived
from its content, so the same lock file gives the same bytes.

Usage, from the repository root:

    python3 .github/scripts/sbom.py OUTPUT.cdx.json

Standard library only, Python 3.9 or newer.
"""

import hashlib
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import third_party_notices as notices  # noqa: E402


def lock_checksums():
    """(name, version) -> the SHA-256 checksum of the crate in compiler/Cargo.lock."""
    path = os.path.join(notices.WORKSPACE, "Cargo.lock")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    sums = {}
    for block in text.split("[[package]]")[1:]:
        fields = dict(re.findall(r'^(name|version|checksum) = "([^"]*)"$', block, re.M))
        if "checksum" in fields:
            sums[(fields["name"], fields["version"])] = fields["checksum"]
    return sums


def toolchain():
    path = os.path.join(notices.WORKSPACE, "rust-toolchain.toml")
    with open(path, encoding="utf-8") as f:
        match = re.search(r'^channel = "([^"]+)"$', f.read(), re.M)
    return match.group(1) if match else "unknown"


def purl(package):
    return "pkg:cargo/{}@{}".format(package["name"], package["version"])


def render(data):
    sums = lock_checksums()
    rows = sorted(notices.shipped(data), key=lambda pc: (pc[0]["name"], pc[0]["version"]))
    shipped_ids = {p["id"] for p, _ in rows}
    members = {p["id"]: p for p in data["packages"] if p["id"] in data["workspace_members"]}
    unio = next(p for p in members.values() if p["name"] == "unio")
    rust = "rust-std@" + toolchain()
    components = [{
        "type": "library",
        "bom-ref": rust,
        "name": "rust-std",
        "version": toolchain(),
        "description": "The Rust standard library of the pinned toolchain "
                       "(compiler/rust-toolchain.toml)",
        "licenses": [{"expression": "MIT OR Apache-2.0"}],
        "externalReferences": [{"type": "vcs", "url": "https://github.com/rust-lang/rust"}],
    }]
    for package, condition in rows:
        component = {
            "type": "library",
            "bom-ref": purl(package),
            "name": package["name"],
            "version": package["version"],
            "purl": purl(package),
        }
        if package.get("license"):
            component["licenses"] = [{"expression": package["license"]}]
        checksum = sums.get((package["name"], package["version"]))
        if checksum:
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        if package.get("repository"):
            component["externalReferences"] = [{"type": "vcs", "url": package["repository"]}]
        targets = "all" if condition is None else ", ".join(sorted(condition))
        component["properties"] = [{"name": "unio:targets", "value": targets}]
        components.append(component)
    nodes = {n["id"]: n for n in data["resolve"]["nodes"]}
    packages = {p["id"]: p for p in data["packages"]}

    def ref(package_id):
        if package_id in members:
            return members[package_id]["name"]
        return purl(packages[package_id])

    dependencies = []
    for package_id in sorted(set(members) | shipped_ids, key=ref):
        depends = set()
        for dep in nodes[package_id]["deps"]:
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            if normal and (dep["pkg"] in shipped_ids or dep["pkg"] in members):
                depends.add(ref(dep["pkg"]))
        if package_id in members:
            depends.add(rust)
        dependencies.append({"ref": ref(package_id), "dependsOn": sorted(depends)})
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tools": {"components": [{
                "type": "application",
                "name": ".github/scripts/sbom.py",
                "description": "Unio's SBOM generator, from cargo metadata and Cargo.lock",
            }]},
            "component": {
                "type": "application",
                "bom-ref": "unio",
                "name": "unio",
                "version": unio["version"],
                "description": "The Unio compiler and its runtime library libunio.a",
                "licenses": [{"expression": unio.get("license") or "MIT OR Apache-2.0"}],
            },
        },
        "components": components,
        "dependencies": dependencies,
    }
    body = json.dumps(bom, indent=2, sort_keys=True)
    digest = hashlib.sha256(body.encode("utf-8")).hexdigest()
    serial = "urn:uuid:{}-{}-5{}-8{}-{}".format(
        digest[0:8], digest[8:12], digest[13:16], digest[17:20], digest[20:32])
    bom["serialNumber"] = serial
    return json.dumps(bom, indent=2, sort_keys=True) + "\n"


def main(argv):
    if len(argv) != 2:
        print("usage: sbom.py OUTPUT.cdx.json", file=sys.stderr)
        return 2
    text = render(notices.metadata())
    with open(argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("wrote {}".format(argv[1]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
