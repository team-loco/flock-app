IBM Plex is bundled unmodified under the SIL OFL in `OFL.txt`.
The files are identical to the scaffold commit (`47bd9f0`), before #93.
SHA-256 checksums were verified against the official IBM release assets:

- Mono: [ibm-plex-mono.zip](https://github.com/IBM/plex/releases/download/%40ibm/plex-mono%402.5.0/ibm-plex-mono.zip)
- Sans: [TrueType.zip](https://github.com/IBM/plex/releases/download/v6.4.0/TrueType.zip)

| File | SHA-256 |
|---|---|
| IBMPlexMono-Regular.ttf | 7c6fbddca4b700be918f5f6183d9bd4464fa427fe435f0b480d77fe2bb8c5a43 |
| IBMPlexMono-Medium.ttf | 98fbd727aae340b236955879dabed4d991aac9e8e90b3b2a67ce4a59221cc97c |
| IBMPlexSans-Regular.ttf | 975dcda37d80f038dcd143c22e33ca2d97a0cc5a929aace1c749153b0fe1afa5 |
| IBMPlexSans-Medium.ttf | 331c8639d7598b2cde62a911a71db195e30cb655cd6bdf2e324a7e984955f907 |
| IBMPlexSans-SemiBold.ttf | a20caf8286023a6a7a85e40b1d2a4ae9fc3e3b1f9eda8f4c542dd4986af67bb1 |

Flock Status is an original geometric symbol font owned by Flock contributors,
licensed under the repository's [Apache-2.0 license](../../../../LICENSE).
`FlockStatus.ttf` and `FlockStatus-Medium.ttf` contain only the six status
icons, plus the required empty `.notdef` glyph and an empty `m`, because gpui
skips any font without an `m` glyph:

| Codepoint | Status | Shape |
|---|---|---|
| ✕ U+2715 | error | filled diamond |
| ▲ U+25B2 | warning | outlined triangle with a mark |
| ● U+25CF | ready | filled dot, radius 3.6 |
| ◐ U+25D0 | progressing | 270° arc, open at the top left |
| ‖ U+2016 | suspended | two bars |
| ○ U+25CB | offline or unknown | dashed circle, radius 5.75, 2.1 on and 2.1 off |

The outlines come from the 16px icon paths in the design: each grid unit is
1/16 em, strokes are 1.6 units wide in both weights, with round caps and joins,
and the generator expands them into filled outlines. Every glyph has a 1em
advance, so each icon is a square, and sits 0.14em below the baseline. The
fonts match Plex Mono's units per em and vertical metrics without copying its
outlines or name tables.

Run `mise run gen:glyphs` to rebuild both files from scratch using uv and fonttools.
The generator reads only numeric metrics from the unmodified Mono fonts and
uses the workspace package version for its name table. Fixed timestamps make
repeated generation produce identical bytes.
