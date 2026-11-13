#!/usr/bin/env python3
"""Writes or checks the Unicode tables of the runtime: compiler/unio-runtime/src/externs/
unicode_tables.rs, which `std.text.normalize_nfc`, `normalize_nfd` and `graphemes` use
(requirement S-18, docs/language-contracts.md section 4).

The data is the Unicode Character Database of one pinned version, UNICODE_VERSION. It is read
through Perl's Unicode::UCD, which ships the UCD of its Unicode version and is part of every
Perl 5 installation, so the generator needs no download: the script refuses a Perl whose
Unicode version is not the pinned one. Taken from the UCD:

- Canonical_Combining_Class (nonzero values, as ranges);
- the canonical decompositions of Decomposition_Mapping (Decomposition_Type=Canonical), one
  step each; Hangul syllables are decomposed by the algorithm of the standard (3.12), not
  by a table;
- the primary composites: the canonical pairs whose character is not Full_Composition_Exclusion;
- Grapheme_Cluster_Break and Extended_Pictographic, for the extended grapheme clusters of
  UAX #29 of that version.

The licence of the data (Unicode License v3) and its version are recorded in
THIRD-PARTY-NOTICES by .github/scripts/third_party_notices.py, which reads the version from the
generated file.

The tables are checked against Python's unicodedata before they are written: the full
canonical decomposition and the composition of every code point assigned in the pinned version
must agree with unicodedata.normalize. Normalization of assigned characters is stable across
Unicode versions, so a newer Python is a valid reference.

Usage, from the repository root:

    python3 .github/scripts/unicode_tables.py          # write the file
    python3 .github/scripts/unicode_tables.py --check  # fail if it is stale

Standard library and Perl only, Python 3.9 or newer.
"""

import json
import os
import subprocess
import sys
import unicodedata

UNICODE_VERSION = "15.0.0"

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUTPUT = os.path.join(ROOT, "compiler", "unio-runtime", "src", "externs", "unicode_tables.rs")

PERL = r"""
use strict;
use warnings;
use Unicode::UCD qw(prop_invmap prop_invlist);

sub invlist_json {
    my @list = @_;
    return "[" . join(",", @list) . "]";
}

print "version ", Unicode::UCD::UnicodeVersion(), "\n";

my ($ccc_l, $ccc_m) = prop_invmap("Canonical_Combining_Class");
for my $i (0 .. $#$ccc_l) {
    my $end = $i < $#$ccc_l ? $ccc_l->[$i + 1] - 1 : 0x10FFFF;
    print "ccc $ccc_l->[$i] $end $ccc_m->[$i]\n" if $ccc_m->[$i] ne "0" && $ccc_m->[$i] ne "Not_Reordered";
}

my ($type_l, $type_m) = prop_invmap("Decomposition_Type");
my ($dm_l, $dm_m, $dm_f) = prop_invmap("Decomposition_Mapping");
die "unexpected format $dm_f" unless $dm_f eq "ad";
sub type_of {
    my ($cp) = @_;
    my ($lo, $hi) = (0, $#$type_l);
    while ($lo < $hi) {
        my $mid = int(($lo + $hi + 1) / 2);
        if ($type_l->[$mid] <= $cp) { $lo = $mid } else { $hi = $mid - 1 }
    }
    return $type_m->[$lo];
}
for my $i (0 .. $#$dm_l) {
    my $value = $dm_m->[$i];
    next if !ref($value) && $value eq "0";
    next if !ref($value) && $value =~ /^</;
    my $end = $i < $#$dm_l ? $dm_l->[$i + 1] - 1 : 0x10FFFF;
    for my $cp ($dm_l->[$i] .. $end) {
        next unless type_of($cp) eq "Canonical";
        my @parts = ref($value) ? @$value : ($value + ($cp - $dm_l->[$i]));
        print "dm $cp ", join(" ", @parts), "\n";
    }
}

print "fce ", invlist_json(prop_invlist("Full_Composition_Exclusion")), "\n";
print "extpict ", invlist_json(prop_invlist("Extended_Pictographic")), "\n";

my ($gcb_l, $gcb_m) = prop_invmap("Grapheme_Cluster_Break");
for my $i (0 .. $#$gcb_l) {
    my $end = $i < $#$gcb_l ? $gcb_l->[$i + 1] - 1 : 0x10FFFF;
    print "gcb $gcb_l->[$i] $end $gcb_m->[$i]\n";
}
"""

# The classes of Grapheme_Cluster_Break the runtime distinguishes, in the order of its enum.
# Perl reports Extended_Pictographic code points of class Other as ExtPict_XX; they are Other
# here, and Extended_Pictographic is a table of its own.
GRAPHEME_CLASSES = [
    "Other",
    "CR",
    "LF",
    "Control",
    "Extend",
    "ZWJ",
    "Regional_Indicator",
    "Prepend",
    "SpacingMark",
    "L",
    "V",
    "T",
    "LV",
    "LVT",
]


def read_ucd():
    result = subprocess.run(
        ["perl", "-e", PERL], stdout=subprocess.PIPE, check=True, universal_newlines=True
    )
    version = None
    ccc = []
    decompositions = {}
    exclusions = []
    extpict = []
    gcb = []
    for line in result.stdout.splitlines():
        kind, _, rest = line.partition(" ")
        if kind == "version":
            version = rest.strip()
        elif kind == "ccc":
            start, end, value = rest.split()
            ccc.append((int(start), int(end), int(value)))
        elif kind == "dm":
            numbers = [int(n) for n in rest.split()]
            decompositions[numbers[0]] = numbers[1:]
        elif kind == "fce":
            exclusions = json.loads(rest)
        elif kind == "extpict":
            extpict = json.loads(rest)
        elif kind == "gcb":
            start, end, value = rest.split()
            if value == "ExtPict_XX":
                value = "Other"
            gcb.append((int(start), int(end), value))
    if version != UNICODE_VERSION:
        sys.exit(
            "Perl's Unicode::UCD has Unicode {}, the tables are pinned to {}".format(
                version, UNICODE_VERSION
            )
        )
    return ccc, decompositions, exclusions, extpict, gcb


def ranges_of(invlist):
    """An inversion list as inclusive ranges."""
    pairs = []
    for i in range(0, len(invlist), 2):
        end = invlist[i + 1] - 1 if i + 1 < len(invlist) else 0x10FFFF
        pairs.append((invlist[i], end))
    return pairs


def in_invlist(invlist, cp):
    inside = False
    for boundary in invlist:
        if boundary > cp:
            break
        inside = not inside
    return inside


def merge(ranges):
    merged = []
    for start, end, value in sorted(ranges):
        if merged and merged[-1][1] + 1 == start and merged[-1][2] == value:
            merged[-1] = (merged[-1][0], end, value)
        else:
            merged.append((start, end, value))
    return merged


HANGUL_S, HANGUL_L, HANGUL_V, HANGUL_T = 0xAC00, 0x1100, 0x1161, 0x11A7
HANGUL_V_COUNT, HANGUL_T_COUNT, HANGUL_S_COUNT = 21, 28, 11172


def full_decomposition(cp, decompositions):
    if HANGUL_S <= cp < HANGUL_S + HANGUL_S_COUNT:
        index = cp - HANGUL_S
        parts = [HANGUL_L + index // (HANGUL_V_COUNT * HANGUL_T_COUNT)]
        parts.append(HANGUL_V + (index % (HANGUL_V_COUNT * HANGUL_T_COUNT)) // HANGUL_T_COUNT)
        if index % HANGUL_T_COUNT:
            parts.append(HANGUL_T + index % HANGUL_T_COUNT)
        return parts
    if cp not in decompositions:
        return [cp]
    out = []
    for part in decompositions[cp]:
        out.extend(full_decomposition(part, decompositions))
    return out


def verify(ccc, decompositions, compositions):
    """Checks the tables against unicodedata for every code point assigned in the pinned
    version (the reference may be newer; stability makes it valid for those)."""
    classes = {}
    for start, end, value in ccc:
        for cp in range(start, end + 1):
            classes[cp] = value
    composite = {(a, b): c for a, b, c in compositions}

    def nfd(cps):
        out = []
        for cp in cps:
            out.extend(full_decomposition(cp, decompositions))
        # canonical ordering
        i = 1
        while i < len(out):
            a, b = classes.get(out[i - 1], 0), classes.get(out[i], 0)
            if b and a > b:
                out[i - 1], out[i] = out[i], out[i - 1]
                i = max(i - 1, 1)
            else:
                i += 1
        return out

    def compose(a, b):
        if HANGUL_L <= a < HANGUL_L + 19 and HANGUL_V <= b < HANGUL_V + HANGUL_V_COUNT:
            return HANGUL_S + ((a - HANGUL_L) * HANGUL_V_COUNT + b - HANGUL_V) * HANGUL_T_COUNT
        if (
            HANGUL_S <= a < HANGUL_S + HANGUL_S_COUNT
            and (a - HANGUL_S) % HANGUL_T_COUNT == 0
            and HANGUL_T < b < HANGUL_T + HANGUL_T_COUNT
        ):
            return a + b - HANGUL_T
        return composite.get((a, b))

    def nfc(cps):
        out = []
        starter = None
        last_class = None
        for cp in nfd(cps):
            klass = classes.get(cp, 0)
            if starter is not None and (last_class is None or last_class < klass):
                combined = compose(out[starter], cp)
                if combined is not None:
                    out[starter] = combined
                    continue
            if klass == 0:
                starter = len(out)
                last_class = None
            else:
                last_class = klass
            out.append(cp)
        return out

    checked = 0
    for cp in range(0x110000):
        if 0xD800 <= cp < 0xE000:
            continue
        if unicodedata.category(chr(cp)) == "Cn":
            continue
        text = chr(cp)
        expected_d = [ord(c) for c in unicodedata.normalize("NFD", text)]
        expected_c = [ord(c) for c in unicodedata.normalize("NFC", text)]
        if nfd([cp]) != expected_d:
            sys.exit("NFD of U+{:04X} disagrees with unicodedata".format(cp))
        if nfc([cp]) != expected_c:
            sys.exit("NFC of U+{:04X} disagrees with unicodedata".format(cp))
        if expected_d != [cp] and nfc(expected_d) != expected_c:
            sys.exit("NFC of the NFD of U+{:04X} disagrees with unicodedata".format(cp))
        checked += 1
    return checked


def render(ccc, decompositions, exclusions, extpict, gcb):
    compositions = []
    for cp, parts in sorted(decompositions.items()):
        if len(parts) == 2 and not in_invlist(exclusions, cp):
            compositions.append((parts[0], parts[1], cp))
    compositions.sort()
    verify(ccc, decompositions, compositions)
    lines = [
        "//! Unicode tables for normalization and grapheme clusters, generated from the Unicode",
        "//! Character Database {} by .github/scripts/unicode_tables.py. Do not edit by".format(
            UNICODE_VERSION
        ),
        "//! hand; the licence of the data is recorded in THIRD-PARTY-NOTICES.",
        "",
        "/// The version of the Unicode Character Database the tables come from.",
        'pub const UNICODE_VERSION: &str = "{}";'.format(UNICODE_VERSION),
        "",
        "/// Nonzero Canonical_Combining_Class values: (first, last, class), sorted.",
        "pub static COMBINING_CLASS: &[(u32, u32, u8)] = &[",
    ]
    for start, end, value in merge(ccc):
        lines.append("    (0x{:X}, 0x{:X}, {}),".format(start, end, value))
    lines.append("];")
    lines.append("")
    lines.append(
        "/// One step of each canonical decomposition, except Hangul syllables: (character,"
    )
    lines.append("/// first, second or 0 for a singleton), sorted.")
    lines.append("pub static DECOMPOSITION: &[(u32, u32, u32)] = &[")
    for cp, parts in sorted(decompositions.items()):
        if len(parts) not in (1, 2):
            sys.exit("a canonical decomposition of U+{:04X} has {} parts".format(cp, len(parts)))
        second = parts[1] if len(parts) == 2 else 0
        lines.append("    (0x{:X}, 0x{:X}, 0x{:X}),".format(cp, parts[0], second))
    lines.append("];")
    lines.append("")
    lines.append(
        "/// The primary composites, except Hangul syllables: (first, second, composite), sorted."
    )
    lines.append("pub static COMPOSITION: &[(u32, u32, u32)] = &[")
    for first, second, cp in compositions:
        lines.append("    (0x{:X}, 0x{:X}, 0x{:X}),".format(first, second, cp))
    lines.append("];")
    lines.append("")
    lines.append("/// Grapheme_Cluster_Break classes, numbered as in `GRAPHEME_CLASS`.")
    lines.append("pub const GRAPHEME_CLASS_NAMES: [&str; {}] = [".format(len(GRAPHEME_CLASSES)))
    for name in GRAPHEME_CLASSES:
        lines.append('    "{}",'.format(name))
    lines.append("];")
    lines.append("")
    lines.append(
        "/// Grapheme_Cluster_Break other than Other: (first, last, class), sorted; Other elsewhere."
    )
    lines.append("pub static GRAPHEME_CLASS: &[(u32, u32, u8)] = &[")
    for start, end, value in merge([r for r in gcb if r[2] != "Other"]):
        if value not in GRAPHEME_CLASSES:
            sys.exit("unknown Grapheme_Cluster_Break value {}".format(value))
        lines.append(
            "    (0x{:X}, 0x{:X}, {}),".format(start, end, GRAPHEME_CLASSES.index(value))
        )
    lines.append("];")
    lines.append("")
    lines.append("/// Extended_Pictographic: (first, last), sorted.")
    lines.append("pub static EXTENDED_PICTOGRAPHIC: &[(u32, u32)] = &[")
    for start, end in ranges_of(extpict):
        lines.append("    (0x{:X}, 0x{:X}),".format(start, end))
    lines.append("];")
    return "\n".join(lines) + "\n"


def main():
    check = "--check" in sys.argv[1:]
    text = render(*read_ucd())
    if check:
        with open(OUTPUT, encoding="utf-8") as f:
            if f.read() != text:
                sys.exit("{} is stale: run {}".format(os.path.relpath(OUTPUT, ROOT), __file__))
        return
    with open(OUTPUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


if __name__ == "__main__":
    main()
