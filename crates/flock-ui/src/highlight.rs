use flock_core::app_settings::Highlight;
use gpui::{Hsla, Rgba, hsla, rgb};

use crate::theme::{Appearance, Palette};

pub const TEXT_CONTRAST: f32 = 4.5;
pub const GLYPH_CONTRAST: f32 = 3.;

const DEGREES: f32 = 360.;
const SOURCE_HUES: [f32; 8] = [215., 35., 150., 290., 185., 330., 80., 255.];
pub const SOURCE_COLORS: usize = SOURCE_HUES.len();
const SOURCE_SATURATION: f32 = 0.7;
const LIGHT_EDITOR: f32 = 0.5;
const PURPLE: (f32, f32) = (265., 0.85);
const TEAL: (f32, f32) = (182., 0.7);
const GREEN: (f32, f32) = (145., 0.6);
const ORANGE: (f32, f32) = (28., 0.9);
const PINK: (f32, f32) = (330., 0.8);
const GRAPHITE: (f32, f32) = (215., 0.12);

const STEP: f32 = 0.01;
const MAX_STEPS: usize = 100;
const LIGHTEST: f32 = 0.98;
const DARKEST: f32 = 0.04;

const DARK_ACC: f32 = 0.73;
const LIGHT_ACC: f32 = 0.47;
const LIGHT_ACC_SATURATION: f32 = 0.74;
const INK_SATURATION: f32 = 0.45;
const INK: f32 = 0.08;
const WHITE: u32 = 0x00FF_FFFF;
const DARK_ACCBG: f32 = 0.12;
const LIGHT_ACCBG: f32 = 0.08;

const DARK_SEL: (f32, f32) = (0.35, 0.19);
const DARK_SELLINE: (f32, f32) = (0.41, 0.39);
const DARK_SELFG: (f32, f32) = (0.6, 0.97);
const LIGHT_SEL: (f32, f32) = (0.77, 0.92);
const LIGHT_SELLINE: (f32, f32) = (0.69, 0.74);
const LIGHT_SELFG: (f32, f32) = (0.72, 0.18);
pub const TEXTSEL_DISTINCT: f32 = 1.2;

const LINEAR_KNEE: f32 = 0.039_28;
const LINEAR_SLOPE: f32 = 12.92;
const GAMMA_OFFSET: f32 = 0.055;
const GAMMA_SCALE: f32 = 1.055;
const GAMMA: f32 = 2.4;
const RED: f32 = 0.2126;
const GREEN_WEIGHT: f32 = 0.7152;
const BLUE: f32 = 0.0722;
const FLARE: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seed {
    hue: f32,
    saturation: f32,
}

fn seed(highlight: Highlight) -> Option<Seed> {
    let preset = |(degrees, saturation): (f32, f32)| Seed {
        hue: degrees / DEGREES,
        saturation,
    };
    match highlight {
        Highlight::Blue => None,
        Highlight::Purple => Some(preset(PURPLE)),
        Highlight::Teal => Some(preset(TEAL)),
        Highlight::Green => Some(preset(GREEN)),
        Highlight::Orange => Some(preset(ORANGE)),
        Highlight::Pink => Some(preset(PINK)),
        Highlight::Graphite => Some(preset(GRAPHITE)),
        Highlight::Custom(color) => {
            let color: Hsla = rgb(color.to_u32()).into();
            Some(Seed {
                hue: color.h,
                saturation: color.s,
            })
        }
    }
}

fn channel(value: f32) -> f32 {
    if value <= LINEAR_KNEE {
        value / LINEAR_SLOPE
    } else {
        ((value + GAMMA_OFFSET) / GAMMA_SCALE).powf(GAMMA)
    }
}

fn luminance(color: Rgba) -> f32 {
    RED * channel(color.r) + GREEN_WEIGHT * channel(color.g) + BLUE * channel(color.b)
}

pub fn over(top: Hsla, bottom: Hsla) -> Hsla {
    let front = Rgba::from(top);
    let back = Rgba::from(bottom);
    let mix = |a: f32, b: f32| a * front.a + b * (1. - front.a);
    Rgba {
        r: mix(front.r, back.r),
        g: mix(front.g, back.g),
        b: mix(front.b, back.b),
        a: 1.,
    }
    .into()
}

pub fn contrast(text: Hsla, ground: Hsla) -> f32 {
    let a = luminance(Rgba::from(text));
    let b = luminance(Rgba::from(ground));
    (a.max(b) + FLARE) / (a.min(b) + FLARE)
}

fn readable(text: Hsla, grounds: &[Hsla], least: f32) -> bool {
    grounds
        .iter()
        .all(|ground| contrast(text, *ground) >= least)
}

fn shade(color: Hsla, toward: Appearance) -> Hsla {
    let lightness = match toward {
        Appearance::Light => (color.l + STEP).min(LIGHTEST),
        Appearance::Dark => (color.l - STEP).max(DARKEST),
    };
    hsla(color.h, color.s, lightness, color.a)
}

fn until(mut color: Hsla, toward: Appearance, done: impl Fn(Hsla) -> bool) -> Hsla {
    for _ in 0..MAX_STEPS {
        if done(color) {
            break;
        }
        color = shade(color, toward);
    }
    color
}

fn tone(seed: Seed, (saturation, lightness): (f32, f32)) -> Hsla {
    hsla(seed.hue, seed.saturation * saturation, lightness, 1.)
}

fn primary(palette: &mut Palette, seed: Seed, appearance: Appearance) {
    let grounds = [palette.editor, palette.panel];
    let ink = hsla(seed.hue, INK_SATURATION, INK, 1.);
    let white: Hsla = rgb(WHITE).into();
    let (start, toward, on, accbg) = match appearance {
        Appearance::Dark => (
            hsla(seed.hue, seed.saturation, DARK_ACC, 1.),
            Appearance::Light,
            ink,
            DARK_ACCBG,
        ),
        Appearance::Light => (
            hsla(
                seed.hue,
                seed.saturation * LIGHT_ACC_SATURATION,
                LIGHT_ACC,
                1.,
            ),
            Appearance::Dark,
            white,
            LIGHT_ACCBG,
        ),
    };
    let editor = palette.editor;
    let acc = until(start, toward, |acc| {
        let tinted = over(acc.opacity(accbg), editor);
        readable(acc, &grounds, TEXT_CONTRAST)
            && contrast(acc, tinted) >= TEXT_CONTRAST
            && contrast(on, acc) >= TEXT_CONTRAST
    });
    palette.acc = acc;
    palette.onacc = on;
    palette.accbg = acc.opacity(accbg);
}

fn secondary(palette: &mut Palette, seed: Seed, appearance: Appearance) {
    let (sel, selline, selfg, toward, away) = match appearance {
        Appearance::Dark => (
            tone(seed, DARK_SEL),
            tone(seed, DARK_SELLINE),
            tone(seed, DARK_SELFG),
            Appearance::Dark,
            Appearance::Light,
        ),
        Appearance::Light => (
            tone(seed, LIGHT_SEL),
            tone(seed, LIGHT_SELLINE),
            tone(seed, LIGHT_SELFG),
            Appearance::Light,
            Appearance::Dark,
        ),
    };
    let texts = [selfg, palette.fg, palette.fg2, palette.mute];
    let glyphs = [palette.ok, palette.warn, palette.err, palette.sus];
    let sel = until(sel, toward, |sel| {
        texts.iter().all(|t| contrast(*t, sel) >= TEXT_CONTRAST)
            && glyphs.iter().all(|g| contrast(*g, sel) >= GLYPH_CONTRAST)
    });
    let textsel = until(sel, away, |textsel| {
        contrast(textsel, sel) >= TEXTSEL_DISTINCT
    });
    palette.sel = sel;
    palette.textsel_on_sel = textsel;
    palette.selline = selline;
    palette.selfg = selfg;
}

pub fn source_colors(palette: &Palette) -> [Hsla; SOURCE_COLORS] {
    let toward = if palette.editor.l > LIGHT_EDITOR {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    let grounds = [
        palette.editor,
        over(palette.errbg, palette.editor),
        palette.sel,
    ];
    SOURCE_HUES.map(|degrees| {
        let start = hsla(degrees / DEGREES, SOURCE_SATURATION, palette.acc.l, 1.);
        until(start, toward, |color| {
            readable(color, &grounds, TEXT_CONTRAST)
        })
    })
}

pub fn apply(
    mut palette: Palette,
    appearance: Appearance,
    accent: Highlight,
    selection: Highlight,
) -> Palette {
    if let Some(seed) = seed(accent) {
        primary(&mut palette, seed, appearance);
    }
    if let Some(seed) = seed(selection) {
        secondary(&mut palette, seed, appearance);
    }
    palette
}

#[cfg(test)]
mod tests {
    use flock_core::app_settings::Rgb;

    use super::*;

    const TOAST_STATUSES: [flock_core::Status; 6] = [
        flock_core::Status::Ok,
        flock_core::Status::Warn,
        flock_core::Status::Err,
        flock_core::Status::Progress,
        flock_core::Status::Suspended,
        flock_core::Status::Offline,
    ];
    const APPEARANCES: [Appearance; 2] = [Appearance::Dark, Appearance::Light];

    fn custom(r: u8, g: u8, b: u8) -> Highlight {
        Highlight::Custom(Rgb { r, g, b })
    }

    fn choices() -> Vec<Highlight> {
        let mut all = Highlight::PRESETS.to_vec();
        all.extend([
            custom(0xff, 0xff, 0x00),
            custom(0x00, 0x00, 0x22),
            custom(0xff, 0xff, 0xff),
            custom(0x80, 0x80, 0x80),
        ]);
        all
    }

    fn failures(palette: &Palette) -> Vec<String> {
        let p = palette;
        let mut failed = Vec::new();
        let mut check = |what: &str, text: Hsla, ground: Hsla, least: f32| {
            let got = contrast(text, ground);
            if got < least {
                failed.push(format!("{what}: {got:.2} < {least}"));
            }
        };
        check("acc on editor", p.acc, p.editor, TEXT_CONTRAST);
        check("acc on panel", p.acc, p.panel, TEXT_CONTRAST);
        check("onacc on acc", p.onacc, p.acc, TEXT_CONTRAST);
        check("acc glyph on chrome", p.acc, p.chrome, GLYPH_CONTRAST);
        check("acc glyph on hover", p.acc, p.hover, GLYPH_CONTRAST);
        check("selfg on sel", p.selfg, p.sel, TEXT_CONTRAST);
        check("fg on sel", p.fg, p.sel, TEXT_CONTRAST);
        check("fg2 on sel", p.fg2, p.sel, TEXT_CONTRAST);
        check("mute on sel", p.mute, p.sel, TEXT_CONTRAST);
        check("selfg on textsel", p.selfg, p.textsel_on_sel, TEXT_CONTRAST);
        check("fg on textsel", p.fg, p.textsel_on_sel, TEXT_CONTRAST);
        check("fg2 on textsel", p.fg2, p.textsel_on_sel, TEXT_CONTRAST);
        check(
            "textsel distinct from sel",
            p.textsel_on_sel,
            p.sel,
            TEXTSEL_DISTINCT,
        );
        check("err glyph on sel", p.err, p.sel, GLYPH_CONTRAST);
        check("ok glyph on sel", p.ok, p.sel, GLYPH_CONTRAST);
        check("warn glyph on sel", p.warn, p.sel, GLYPH_CONTRAST);
        check(
            "acc on accbg",
            p.acc,
            over(p.accbg, p.editor),
            TEXT_CONTRAST,
        );
        for status in TOAST_STATUSES {
            let ground = p.toast_ground(status);
            check("fg on toast", p.fg, ground, TEXT_CONTRAST);
            check("fg2 on toast", p.fg2, ground, TEXT_CONTRAST);
            check("toast glyph", p.status(status), ground, GLYPH_CONTRAST);
        }
        failed
    }

    fn base(appearance: Appearance) -> Palette {
        Palette::for_appearance(appearance)
    }

    #[test]
    fn every_preset_and_hard_custom_colour_is_legible_in_both_modes() {
        for appearance in APPEARANCES {
            for accent in choices() {
                for selection in choices() {
                    let palette = apply(base(appearance), appearance, accent, selection);
                    let failed = failures(&palette);
                    assert!(
                        failed.is_empty(),
                        "{appearance:?} {accent:?}/{selection:?}: {failed:#?}"
                    );
                }
            }
        }
    }

    #[test]
    fn pod_colours_are_legible_and_distinct_in_every_mode_and_highlight() {
        for appearance in APPEARANCES {
            for accent in choices() {
                for selection in choices() {
                    let palette = apply(base(appearance), appearance, accent, selection);
                    let colors = source_colors(&palette);
                    let grounds = [
                        palette.editor,
                        over(palette.errbg, palette.editor),
                        palette.sel,
                    ];
                    for color in colors {
                        for ground in grounds {
                            let got = contrast(color, ground);
                            assert!(
                                got >= TEXT_CONTRAST,
                                "{appearance:?} {accent:?}/{selection:?}: {got:.2}"
                            );
                        }
                    }
                    for (ix, a) in colors.iter().enumerate() {
                        for b in colors.iter().skip(ix + 1) {
                            assert!((a.h - b.h).abs() > f32::EPSILON);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn blue_keeps_the_design_tokens() {
        for appearance in APPEARANCES {
            let palette = apply(
                base(appearance),
                appearance,
                Highlight::Blue,
                Highlight::Blue,
            );
            assert_eq!(palette, base(appearance));
        }
    }

    #[test]
    fn the_two_highlights_touch_only_their_own_tokens() {
        let appearance = Appearance::Dark;
        let before = base(appearance);
        let accent = apply(before, appearance, Highlight::Orange, Highlight::Blue);
        assert_ne!(accent.acc, before.acc);
        assert_eq!((accent.sel, accent.selfg), (before.sel, before.selfg));
        assert_eq!((accent.fg, accent.editor), (before.fg, before.editor));
        let selection = apply(before, appearance, Highlight::Blue, Highlight::Green);
        assert_ne!(selection.sel, before.sel);
        assert_ne!(selection.selline, before.selline);
        assert_eq!((selection.acc, selection.onacc), (before.acc, before.onacc));
    }

    #[test]
    fn one_hue_gives_a_dark_and_a_light_variant() {
        let dark = apply(
            base(Appearance::Dark),
            Appearance::Dark,
            Highlight::Purple,
            Highlight::Purple,
        );
        let light = apply(
            base(Appearance::Light),
            Appearance::Light,
            Highlight::Purple,
            Highlight::Purple,
        );
        assert!(dark.acc.l > light.acc.l);
        assert!(dark.sel.l < light.sel.l);
        assert!((dark.acc.h - light.acc.h).abs() < f32::EPSILON);
    }

    #[test]
    fn an_unreadable_custom_colour_is_shaded_until_it_passes() {
        let yellow = custom(0xff, 0xee, 0x00);
        let light = apply(
            base(Appearance::Light),
            Appearance::Light,
            yellow,
            Highlight::Blue,
        );
        let start = hsla(light.acc.h, light.acc.s, LIGHT_ACC, 1.);
        assert!(contrast(start, light.editor) < TEXT_CONTRAST);
        assert!(contrast(light.acc, light.editor) >= TEXT_CONTRAST);
        assert!(light.acc.l < LIGHT_ACC);
    }

    #[test]
    fn contrast_matches_the_wcag_reference_values() {
        let black: Hsla = rgb(0x0000_0000).into();
        let white: Hsla = rgb(WHITE).into();
        assert!((contrast(black, white) - 21.).abs() < 0.01);
        assert!((contrast(white, white) - 1.).abs() < 0.01);
        let dark = base(Appearance::Dark);
        assert!((contrast(dark.fg, dark.editor) - 13.92).abs() < 0.05);
    }
}
