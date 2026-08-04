#!/usr/bin/env python3
"""Generate static B2-B0 hostile-byte fixtures; this is not a diff parser."""

import argparse
import os
import tempfile
from hashlib import sha256
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
COMMITTED = ROOT / "docs" / "fixtures" / "phase-3c-b2-b0" / "raw"
BASE = (
    b"diff --git a/file.txt b/file.txt\n"
    b"index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n"
    b"--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-a\n"
)


def body(payload: bytes) -> bytes:
    return BASE + b"+" + payload + b"\n"


FIXTURES = {
    "N20-utf8-lone-continuation.bin": body(b"\x80"),
    "N21-utf8-truncated.bin": body(b"\xe2\x82"),
    "N22-utf8-overlong.bin": body(b"\xc0\xaf"),
    "N23-utf8-surrogate.bin": body(b"\xed\xa0\x80"),
    "N24-utf8-above-range.bin": body(b"\xf4\x90\x80\x80"),
    "N25-utf8-section.bin": BASE.replace(b"@@ -1 +1 @@", b"@@ -1 +1 @@ \xff"),
    "N26-utf8-path.bin": BASE.replace(b"a/file.txt", b"a/\xff.txt", 1),
    "N27-nul-payload.bin": body(b"x\x00y"),
    "N28-nul-path.bin": BASE.replace(b"a/file.txt", b"a/fi\x00le.txt", 1),
    "N29-nul-section.bin": BASE.replace(b"@@ -1 +1 @@", b"@@ -1 +1 @@ x\x00"),
    "N30-nul-between-records.bin": BASE + b"\x00+new\n",
    "N31-nul-trailing.bin": body(b"new") + b"\x00",
    "N32-esc-lone.bin": body(b"x\x1by"),
    "N33-esc-csi.bin": body(b"\x1b[31mred\x1b[0m"),
    "N34-esc-header.bin": BASE.replace(b"diff --git", b"\x1b[31mdiff --git", 1),
    "N35-esc-hunk.bin": BASE.replace(b"@@ -1 +1 @@", b"\x1b[31m@@ -1 +1 @@", 1),
    "N36-esc-osc.bin": body(b"\x1b]0;title\x07x"),
    "N37-c0-01.bin": body(b"x\x01"),
    "N38-c0-07.bin": body(b"x\x07"),
    "N39-c0-08.bin": body(b"x\x08"),
    "N40-c0-0b.bin": body(b"x\x0b"),
    "N41-c0-0c.bin": body(b"x\x0c"),
    "N42-c0-0e.bin": body(b"x\x0e"),
    "N43-c0-1f.bin": body(b"x\x1f"),
    "N44-bidi-202a.bin": body("x\u202a".encode()),
    "N45-bidi-202b.bin": body("x\u202b".encode()),
    "N46-bidi-202d.bin": body("x\u202d".encode()),
    "N47-bidi-202e.bin": body("x\u202e".encode()),
    "N48-bidi-202c.bin": body("x\u202c".encode()),
    "N49-bidi-2066.bin": body("x\u2066".encode()),
    "N50-bidi-2067.bin": body("x\u2067".encode()),
    "N51-bidi-2068.bin": body("x\u2068".encode()),
    "N52-bidi-2069.bin": body("x\u2069".encode()),
    "N53-bidi-section.bin": BASE.replace(b"@@ -1 +1 @@", "@@ -1 +1 @@ \u202e".encode()),
    "N54-bidi-path.bin": BASE.replace(b"a/file.txt", "a/\u202efile.txt".encode(), 1),
    "P13-crlf-valid.bin": body(b"new\r"),
    "N55-cr-interior.bin": body(b"n\rew"),
    "N56-cr-hunk.bin": BASE.replace(b"@@ -1 +1 @@", b"@@ -1\r +1 @@"),
    "N57-cr-diff.bin": BASE.replace(b"diff --git", b"diff\r --git", 1),
    "N58-cr-path.bin": BASE.replace(b"a/file.txt", b"a/fi\rle.txt", 1),
    "N59-cr-eof.bin": BASE + b"+new\r",
    "N60-cr-double-terminal.bin": body(b"new\r\r"),
    "N61-cr-marker.bin": BASE + b"+new\n\\ No new\rline at end of file\n",
    "N62-cr-empty-payload.bin": body(b"\r"),
}


def require_fresh_output(value: str) -> Path:
    output = Path(value).expanduser().resolve(strict=False)
    if output == ROOT or output == COMMITTED or ROOT in output.parents:
        raise SystemExit("refusing repository output path")
    if output.exists():
        if output.is_symlink() or not output.is_dir() or any(output.iterdir()):
            raise SystemExit("output directory must be a fresh empty non-symlink directory")
    else:
        output.mkdir(parents=True)
    temporary_roots = {Path(tempfile.gettempdir()).resolve(), Path("/private/tmp").resolve()}
    if not any(root == output.resolve() or root in output.resolve().parents for root in temporary_roots):
        raise SystemExit("output directory must be beneath the system temporary directory")
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--compare-to", type=Path)
    args = parser.parse_args()
    output = require_fresh_output(args.output_dir)
    manifest = output.parent / "raw-manifest.tsv"
    rows = [
        "# id\tpath\tsha256\tbytes\tclass\tprovenance\texpected\tdangerous-offset\trationale"
    ]
    for name, data in FIXTURES.items():
        (output / name).write_bytes(data)
        fixture_id = name.removesuffix(".bin")
        expected = "Extractable(LineEnding::CrLf)" if fixture_id == "P13-crlf-valid" else "MalformedOutput"
        offset = next((index for index, byte in enumerate(data) if byte >= 0x80 or byte in (0, 1, 7, 8, 11, 12, 13, 14, 27, 31)), -1)
        rows.append(
            "\t".join(
                (
                    fixture_id,
                    f"raw/{name}",
                    sha256(data).hexdigest(),
                    str(len(data)),
                    "positive" if fixture_id.startswith("P") else "negative",
                    "manual-adversarial",
                    expected,
                    str(offset),
                    "byte-exact hostile input generated from reviewed Python bytes",
                )
            )
        )
    manifest.write_text("\n".join(rows) + "\n", encoding="ascii")
    if args.compare_to:
        target = args.compare_to.resolve(strict=True)
        if target != COMMITTED:
            raise SystemExit("comparison target must be the committed raw fixture directory")
        for name, data in FIXTURES.items():
            if (target / name).read_bytes() != data:
                raise SystemExit(f"comparison mismatch: {name}")
    print(f"generated {len(FIXTURES)} raw B2-B0 fixtures in {output}")


if __name__ == "__main__":
    main()
