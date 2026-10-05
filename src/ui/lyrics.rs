//! The words of the playing track, in a side panel that follows the song.

use egui::emath::TSTransform;
use egui::{Align, Color32, Frame, Layout, Margin, Rect, Sense, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::i18n::{gettext, pgettext};
use crate::model::{Action, Loadable};
use crate::player::RepeatMode;
use crate::theme::{self, Icon};

use super::lyrics_fx::{self, Glow, light};
use super::widgets;

const LINE_SIZE: f32 = 22.0;
const LINE_GAP: f32 = 12.0;
/// Where the line being sung sits, as a fraction of the visible lyrics from
/// the top: high up, so the lines to come fill most of the view.
const SUNG_LINE_AT: f32 = 0.2;

/// Scrolls so the middle of `line` sits `SUNG_LINE_AT` of the way down the
/// visible lyrics.
fn show_sung_line(ui: &egui::Ui, line: Rect, animation: Option<egui::style::ScrollAnimation>) {
    let above = (ui.clip_rect().height() * SUNG_LINE_AT - line.height() / 2.0).max(0.0);
    let target = Rect::from_min_max(pos2(line.left(), line.top() - above), line.max);
    match animation {
        Some(animation) => ui.scroll_to_rect_animation(target, Some(Align::Min), animation),
        None => ui.scroll_to_rect(target, Some(Align::Min)),
    }
}
/// How long a line takes to light up or fade.
const LIGHT_UP_SECONDS: f32 = 0.22;

// Lines are drawn in layers, the way Beautiful Lyrics draws them: every
// line sits at half strength, the one being sung brightens from left to
// right as it is sung with a soft glow behind the sung words, and lines
// further from it fade a little more for depth. Gaps in the singing show
// three dots that fill in time with the gap.

/// How strong a line reads before and after it is sung.
const IDLE_OPACITY: f32 = 0.5;
/// How much fainter each line is per line of distance from the sung one.
const DISTANCE_FADE: f32 = 0.07;
/// The faintest a distant line gets.
const FARTHEST_OPACITY: f32 = 0.2;
/// Strips that soften the leading edge of the sweep.
const SWEEP_STRIPS: usize = 6;
/// The shortest and longest time a sweep takes when the words carry only
/// line starts: long lines take longer, and a pause before the next line
/// does not drag the sweep out.
const SWEEP_MIN_MS: u32 = 1_200;
const SWEEP_PER_CHAR_MS: u32 = 80;
/// A pause this long before the first line shows the interlude dots.
const INTRO_DOTS_MS: u32 = 2_500;
/// How small a line waits, in full screen, before it grows to be sung.
const IDLE_SCALE: f32 = 0.9;
/// How long a word takes to swell as it starts, and to settle after it.
const POP_IN_MS: f32 = 140.0;
const POP_OUT_MS: f32 = 420.0;
/// A word held longer than this is a long note, and swells further.
const LONG_NOTE_MS: f32 = 700.0;

/// A zoom by `scale` that leaves `pivot` where it is.
fn zoom_about(pivot: egui::Pos2, scale: f32) -> TSTransform {
    TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale)
}

/// Paints `galley` at `pos` as laid out, then moved and scaled by `zoom`.
fn paint_galley(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: &std::sync::Arc<egui::Galley>,
    color: Color32,
    zoom: TSTransform,
) {
    if zoom == TSTransform::IDENTITY {
        painter.galley_with_override_text_color(pos, galley.clone(), color);
        return;
    }
    let mut shape =
        egui::epaint::TextShape::new(pos, galley.clone(), color).with_override_text_color(color);
    shape.transform(zoom);
    painter.add(shape);
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What one row of the lyrics shows this frame.
#[derive(Clone, Copy)]
struct Look {
    color: Color32,
    /// From 0 to 1 as the line becomes the one being sung.
    lit: f32,
    /// How much of the line has been sung, from 0 to 1.
    sung: f32,
    /// Lines between this one and the one being sung.
    distance: usize,
    /// The song's pulse, in full screen: the line grows as it is sung, and
    /// its words swell and glow in the cover's colour.
    glow: Option<Glow>,
}

fn idle_opacity(distance: usize, hovered: bool) -> f32 {
    if hovered {
        return 0.72;
    }
    (IDLE_OPACITY - DISTANCE_FADE * distance.saturating_sub(1) as f32).max(FARTHEST_OPACITY)
}

/// How bright the glow behind the sung words is: it rises over the first
/// half of the line, holds, and lets go as the line ends.
fn glow(sung: f32) -> f32 {
    if sung < 0.5 {
        smooth(sung / 0.5)
    } else if sung < 0.925 {
        1.0
    } else {
        1.0 - smooth((sung - 0.925) / 0.075)
    }
}

/// When line `index` has been sung through, as a fraction of its sweep.
fn sung_fraction(lyrics: &crate::lyrics::Lyrics, index: usize, position_ms: u32) -> f32 {
    let line = &lyrics.lines[index];
    let Some(at) = line.at_ms else {
        return 1.0;
    };
    let next = lyrics.lines[index + 1..]
        .iter()
        .find_map(|line| line.at_ms)
        .unwrap_or(u32::MAX);
    let chars = line.text.chars().count() as u32;
    let sweep = (SWEEP_MIN_MS.max(chars * SWEEP_PER_CHAR_MS)).min(next.saturating_sub(at).max(1));
    (position_ms.saturating_sub(at) as f32 / sweep as f32).clamp(0.0, 1.0)
}

/// How far through the gap that line `index` stands for (an empty line, or
/// the intro before the first line when `index` is `None`) the song is.
fn gap_fraction(lyrics: &crate::lyrics::Lyrics, index: Option<usize>, position_ms: u32) -> f32 {
    let (start, after) = match index {
        Some(index) => (lyrics.lines[index].at_ms.unwrap_or(0), index + 1),
        None => (0, 0),
    };
    let end = lyrics.lines[after..]
        .iter()
        .find_map(|line| line.at_ms)
        .unwrap_or(start + 4_000);
    (position_ms.saturating_sub(start) as f32 / end.saturating_sub(start).max(1) as f32)
        .clamp(0.0, 1.0)
}

/// One word of a line and when it is sung.
struct TimedWord<'a> {
    text: &'a str,
    start: u32,
    end: u32,
}

/// When each word of line `index` is sung: from the lyrics' own word
/// stamps when they have them, otherwise spread over the line's sweep by
/// the length of each word, so every timed line lights word by word.
fn timed_words(lyrics: &crate::lyrics::Lyrics, index: usize) -> Vec<TimedWord<'_>> {
    let line = &lyrics.lines[index];
    let Some(at) = line.at_ms else {
        return Vec::new();
    };
    let next = lyrics.lines[index + 1..]
        .iter()
        .find_map(|line| line.at_ms)
        .unwrap_or(u32::MAX);
    let length = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count() as u32;
    if !line.words.is_empty() {
        let last = line.words.last().map_or(at, |word| word.at_ms);
        let last_length = line.words.last().map_or(0, |word| length(&word.text));
        let end = (last + 400.max(last_length * SWEEP_PER_CHAR_MS)).min(next.max(last + 1));
        return line
            .words
            .iter()
            .enumerate()
            .map(|(i, word)| TimedWord {
                text: &word.text,
                start: word.at_ms,
                end: word
                    .end_ms
                    .or_else(|| line.words.get(i + 1).map(|next| next.at_ms))
                    .unwrap_or(end)
                    .max(word.at_ms + 1),
            })
            .collect();
    }
    let chars = line.text.chars().count() as u32;
    let sweep = (SWEEP_MIN_MS.max(chars * SWEEP_PER_CHAR_MS)).min(next.saturating_sub(at).max(1));
    let pieces: Vec<&str> = line.text.split_inclusive(char::is_whitespace).collect();
    // A small share per word on top of its letters, so short words still
    // get a beat of their own.
    let weight = |text: &str| length(text) + 2;
    let total: u32 = pieces.iter().map(|piece| weight(piece)).sum::<u32>().max(1);
    let mut before = 0;
    pieces
        .into_iter()
        .map(|piece| {
            let start = at + (u64::from(sweep) * u64::from(before) / u64::from(total)) as u32;
            before += weight(piece);
            let end = at + (u64::from(sweep) * u64::from(before) / u64::from(total)) as u32;
            TimedWord {
                text: piece,
                start,
                end: end.max(start + 1),
            }
        })
        .collect()
}

/// A line sung word by word, the way Beautiful Lyrics sings: each word
/// brightens from left to right in its own time, rises a little and glows
/// while it is sung, and settles when the line is over. Left to right
/// text only; right-to-left lines use [`lyric_row`].
fn word_row(
    ui: &mut egui::Ui,
    words: &[TimedWord<'_>],
    font: egui::FontId,
    look: Look,
    position_ms: u32,
    sense: Sense,
) -> egui::Response {
    let width = ui.available_width();
    let painter = ui.painter().clone();
    let galleys: Vec<_> = words
        .iter()
        .map(|word| painter.layout_no_wrap(word.text.to_string(), font.clone(), look.color))
        .collect();
    let row_height = galleys
        .iter()
        .map(|galley| galley.size().y)
        .fold(font.size * 1.2, f32::max);
    // Flow the words into rows, breaking before a word that would not fit.
    let mut places: Vec<egui::Vec2> = Vec::with_capacity(galleys.len());
    // Syllables of one word stay together: a row may only break after a
    // piece that ends in a space.
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut word_start = 0;
    for (index, (word, galley)) in words.iter().zip(&galleys).enumerate() {
        let ink = layout_width(word.text, galley);
        let starts_word = index == 0 || words[index - 1].text.ends_with(char::is_whitespace);
        if starts_word {
            word_start = index;
        }
        if x + ink > width && x > 0.0 {
            if starts_word {
                x = 0.0;
                y += row_height;
            } else if places[word_start].x > 0.0 {
                // Move the whole word down to the next row.
                let shift = places[word_start].x;
                for place in &mut places[word_start..] {
                    place.x -= shift;
                    place.y += row_height;
                }
                x -= shift;
                y += row_height;
            }
        }
        places.push(vec2(x, y));
        x += galley.size().x;
    }
    let height = y + row_height;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = response.hovered() && sense.senses_click();
    let idle = idle_opacity(look.distance, hovered);
    let base = idle + (IDLE_OPACITY.max(idle) - idle) * look.lit;
    let light_text =
        u32::from(look.color.r()) + u32::from(look.color.g()) + u32::from(look.color.b()) > 3 * 128;
    let lift_by = font.size * 0.06;
    // In full screen the line grows from the left as it becomes the one
    // being sung.
    let line_zoom = match look.glow {
        Some(_) => zoom_about(
            pos2(rect.left(), rect.center().y),
            IDLE_SCALE + (1.0 - IDLE_SCALE) * smooth(look.lit),
        ),
        None => TSTransform::IDENTITY,
    };
    // How far through each word the song is, how much the word swells
    // (in full screen, while it is sung, and settling once the next word
    // takes over), and how long a note it is.
    let timing = |word: &TimedWord<'_>| {
        let length = word.end.saturating_sub(word.start).max(1) as f32;
        let sung = (position_ms.saturating_sub(word.start) as f32 / length).clamp(0.0, 1.0);
        let sung = if look.sung >= 1.0 { 1.0 } else { sung };
        let held = ((length - LONG_NOTE_MS) / 1_500.0).clamp(0.0, 1.0);
        let pop = if look.glow.is_some() && position_ms >= word.start && look.sung < 1.0 {
            let since = position_ms.saturating_sub(word.start) as f32;
            let after = position_ms.saturating_sub(word.end) as f32;
            smooth(since / POP_IN_MS)
                * (1.0 - smooth(after / POP_OUT_MS))
                * (1.0 - held * 0.4 * (1.0 - sung))
                * look.lit
        } else {
            0.0
        };
        (sung, pop, held)
    };
    // In full screen the sung words bloom: each row's own letters, blurred,
    // laid behind it as light in the cover's colour. The light follows the
    // sweep and is strongest on the word being sung.
    if let Some(pulse) = look.glow
        && look.lit > 0.01
        && light_text
    {
        use std::hash::{Hash, Hasher};
        let pad = row_height * 0.8;
        let feather = row_height * 0.6;
        let states: Vec<_> = words.iter().map(&timing).collect();
        let strength =
            |index: usize| look.lit * (0.5 + 0.5 * states[index].1) * (0.85 + 0.3 * pulse.bass);
        let color = lyrics_fx::mix(pulse.accent, Color32::WHITE, 0.3);
        let mut start = 0;
        while start < words.len() {
            // The words that share this row.
            let row_y = places[start].y;
            let end = (start..words.len())
                .find(|index| places[*index].y != row_y)
                .unwrap_or(words.len());
            let row = start..end;
            start = end;
            if states[row.start].0 <= 0.0 {
                continue;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for word in &words[row.clone()] {
                word.text.hash(&mut hasher);
            }
            (font.size.to_bits(), width.to_bits(), row.start).hash(&mut hasher);
            let pieces: Vec<_> = row
                .clone()
                .map(|index| (vec2(places[index].x, 0.0), galleys[index].clone()))
                .collect();
            let Some(texture) = lyrics_fx::text_bloom(
                ui.ctx(),
                hasher.finish(),
                vec2(width, row_height),
                pad,
                row_height * 0.2,
                &pieces,
            ) else {
                continue;
            };
            let top = rect.top() + row_y;
            let field = Rect::from_min_size(
                pos2(rect.left() - pad, top - pad),
                vec2(width + 2.0 * pad, row_height + 2.0 * pad),
            );
            let mut mesh = egui::Mesh::with_texture(texture.id());
            for index in row.clone() {
                let sung = states[index].0;
                if sung <= 0.0 {
                    continue;
                }
                // Each word lights its own stretch of the row, out to the
                // next word, so the stretches meet without overlapping.
                let first = index == row.start;
                let last = index + 1 == row.end;
                let left = rect.left() + places[index].x;
                let x0 = if first { left - pad } else { left };
                let x1 = if last {
                    left + galleys[index].size().x + pad
                } else {
                    rect.left() + places[index + 1].x
                };
                let front =
                    left + sung * (layout_width(words[index].text, &galleys[index]) + feather);
                let here = strength(index);
                let before = if first {
                    here
                } else {
                    (here + strength(index - 1)) / 2.0
                };
                let after = if last {
                    here
                } else {
                    (here + strength(index + 1)) / 2.0
                };
                const SLICES: u32 = 8;
                for slice in 0..=SLICES {
                    let t = slice as f32 / SLICES as f32;
                    let x = x0 + (x1 - x0) * t;
                    let level = if t < 0.5 {
                        before + (here - before) * t * 2.0
                    } else {
                        here + (after - here) * (t - 0.5) * 2.0
                    };
                    let reached = if sung >= 1.0 {
                        1.0
                    } else {
                        ((front - x) / feather).clamp(0.0, 1.0)
                    };
                    let color = light(color, level * reached);
                    let at = mesh.vertices.len() as u32;
                    for y in [field.top(), field.bottom()] {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: line_zoom * pos2(x, y),
                            uv: pos2(
                                (x - field.left()) / field.width(),
                                (y - field.top()) / field.height(),
                            ),
                            color,
                        });
                    }
                    if slice > 0 {
                        mesh.add_triangle(at - 2, at - 1, at);
                        mesh.add_triangle(at - 1, at, at + 1);
                    }
                }
            }
            painter.add(mesh);
        }
    }
    for ((word, galley), place) in words.iter().zip(&galleys).zip(&places) {
        let (sung, pop, held) = timing(word);
        // A word rises as it is sung and stays up until the line lets go.
        let eased = sung * sung * (3.0 - 2.0 * sung);
        let lift = lift_by * eased * look.lit;
        let pos = rect.min + *place - vec2(0.0, lift);
        let ink = layout_width(word.text, galley);
        let word_rect = Rect::from_min_size(pos, vec2(ink, galley.size().y));
        let zoom = line_zoom
            * zoom_about(
                pos2(word_rect.center().x, word_rect.bottom()),
                1.0 + (0.025 + 0.045 * held) * pop,
            );
        paint_galley(&painter, pos, galley, look.color.gamma_multiply(base), zoom);
        if look.lit <= 0.001 || sung <= 0.0 {
            continue;
        }
        let bright = look.color.gamma_multiply(look.lit);
        let feather = (row_height * 0.6).min(ink.max(1.0));
        let front = sung * (ink + feather);
        // The panel's glow follows each word and lets go of it; full
        // screen has its bloom instead.
        let glow_alpha = if light_text && look.glow.is_none() {
            glow(sung) * look.lit
        } else {
            0.0
        };
        let solid = Rect::from_x_y_ranges(
            word_rect.left()..=word_rect.left() + (front - feather).clamp(0.0, ink),
            word_rect.top() - row_height * 0.2..=word_rect.bottom() + row_height * 0.2,
        );
        if solid.width() > 0.0 {
            let clipped = painter.with_clip_rect((zoom * solid).intersect(painter.clip_rect()));
            if glow_alpha > 0.01 {
                let halo = look.color.gamma_multiply(0.09 * glow_alpha);
                let radius = row_height * 0.1;
                for step in 0..8 {
                    let angle = step as f32 * std::f32::consts::TAU / 8.0;
                    paint_galley(
                        &clipped,
                        pos + vec2(angle.cos(), angle.sin()) * radius,
                        galley,
                        halo,
                        zoom,
                    );
                }
            }
            paint_galley(&clipped, pos, galley, bright, zoom);
        }
        for strip in 0..SWEEP_STRIPS {
            let from = front - feather + feather * strip as f32 / SWEEP_STRIPS as f32;
            let to = from + feather / SWEEP_STRIPS as f32;
            let (from, to) = (from.clamp(0.0, ink), to.clamp(0.0, ink));
            if to <= from {
                continue;
            }
            let clip = Rect::from_x_y_ranges(
                word_rect.left() + from..=word_rect.left() + to,
                solid.y_range(),
            );
            let alpha = 1.0 - (strip as f32 + 0.5) / SWEEP_STRIPS as f32;
            paint_galley(
                &painter.with_clip_rect((zoom * clip).intersect(painter.clip_rect())),
                pos,
                galley,
                bright.gamma_multiply(alpha),
                zoom,
            );
        }
    }
    response
}

/// The width of a word's letters, leaving out its trailing space.
fn layout_width(text: &str, galley: &egui::Galley) -> f32 {
    let trailing = text.chars().rev().take_while(|c| c.is_whitespace()).count();
    if trailing == 0 {
        return galley.size().x;
    }
    galley
        .rows
        .first()
        .and_then(|row| {
            let glyphs = &row.row.glyphs;
            let last = glyphs.len().checked_sub(trailing + 1)?;
            let glyph = glyphs.get(last)?;
            Some(glyph.pos.x + glyph.advance_width)
        })
        .unwrap_or(galley.size().x)
}

/// Lays out and paints one line of lyrics, the full width of `ui`.
fn lyric_row(
    ui: &mut egui::Ui,
    text: &str,
    font: egui::FontId,
    look: Look,
    sense: Sense,
) -> egui::Response {
    let galley = crate::bidi::layout(
        ui.painter(),
        text,
        font,
        look.color,
        ui.available_width(),
        usize::MAX,
        None,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y), sense);
    if ui.is_rect_visible(rect) {
        paint_lyric(
            ui,
            rect,
            &galley,
            look,
            response.hovered() && sense.senses_click(),
            crate::bidi::is_rtl(text),
        );
    }
    response
}

fn paint_lyric(
    ui: &egui::Ui,
    rect: Rect,
    galley: &std::sync::Arc<egui::Galley>,
    look: Look,
    hovered: bool,
    rtl: bool,
) {
    let pos = crate::bidi::galley_pos(rect, galley);
    let painter = ui.painter();
    let idle = idle_opacity(look.distance, hovered);
    let base = idle + (IDLE_OPACITY.max(idle) - idle) * look.lit;
    // In full screen the line grows, from the side it starts on, as it
    // becomes the one being sung.
    let zoom = match look.glow {
        Some(_) => zoom_about(
            pos2(
                if rtl { rect.right() } else { rect.left() },
                rect.center().y,
            ),
            IDLE_SCALE + (1.0 - IDLE_SCALE) * smooth(look.lit),
        ),
        None => TSTransform::IDENTITY,
    };
    paint_galley(painter, pos, galley, look.color.gamma_multiply(base), zoom);
    if look.lit <= 0.001 {
        return;
    }
    let bright = look.color.gamma_multiply(look.lit);
    let total: f32 = galley.rows.iter().map(|row| row.rect().width()).sum();
    let Some(first) = galley.rows.first() else {
        return;
    };
    if total <= 0.0 {
        return;
    }
    let feather = first.rect().height() * 1.2;
    // Light words on a dark backdrop glow; dark words on a light panel
    // would only smudge.
    let light =
        u32::from(look.color.r()) + u32::from(look.color.g()) + u32::from(look.color.b()) > 3 * 128;
    let glow_alpha = if light {
        glow(look.sung) * look.lit
    } else {
        0.0
    };
    let glow_radius = first.rect().height() * 0.09;
    // The sweep's front, measured along the rows as if they were one line,
    // and run past the end by the feather so a sung line ends fully lit.
    let reach = look.sung * (total + feather);
    let mut before = 0.0;
    for row in &galley.rows {
        let row_rect = row.rect().translate(pos.to_vec2());
        let width = row_rect.width();
        let front = reach - before;
        before += width;
        if front <= 0.0 {
            break;
        }
        let span = |from: f32, to: f32| {
            let (from, to) = (from.clamp(0.0, width), to.clamp(0.0, width));
            let x = if rtl {
                row_rect.right() - to..=row_rect.right() - from
            } else {
                row_rect.left() + from..=row_rect.left() + to
            };
            Rect::from_x_y_ranges(x, row_rect.y_range())
        };
        let solid = span(0.0, front - feather);
        if solid.width() > 0.0 {
            let clipped = painter.with_clip_rect((zoom * solid).intersect(painter.clip_rect()));
            if glow_alpha > 0.01 {
                let halo = look.color.gamma_multiply(0.07 * glow_alpha);
                for step in 0..8 {
                    let angle = step as f32 * std::f32::consts::TAU / 8.0;
                    let offset = vec2(angle.cos(), angle.sin()) * glow_radius;
                    paint_galley(&clipped, pos + offset, galley, halo, zoom);
                }
            }
            paint_galley(&clipped, pos, galley, bright, zoom);
        }
        for strip in 0..SWEEP_STRIPS {
            let from = front - feather + feather * strip as f32 / SWEEP_STRIPS as f32;
            let to = from + feather / SWEEP_STRIPS as f32;
            let clip = span(from, to);
            if clip.width() <= 0.0 {
                continue;
            }
            let alpha = 1.0 - (strip as f32 + 0.5) / SWEEP_STRIPS as f32;
            paint_galley(
                &painter.with_clip_rect((zoom * clip).intersect(painter.clip_rect())),
                pos,
                galley,
                bright.gamma_multiply(alpha),
                zoom,
            );
        }
    }
}

/// Three dots for a gap in the singing, filling one after another through
/// the gap and breathing while they wait. Collapsed when not current.
fn interlude_row(
    ui: &mut egui::Ui,
    color: Color32,
    size: f32,
    lit: f32,
    through: f32,
    pulse: Option<Glow>,
) {
    let height = size * 1.1 * lit;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if lit <= 0.01 || !ui.is_rect_visible(rect) {
        return;
    }
    let time = ui.input(|input| input.time) as f32;
    let radius = size * 0.24;
    let gap = size * 0.74;
    let breathe = 1.0 + 0.07 * (time * 2.6).sin();
    // At the end of the gap the dots grow a little and then shrink away
    // as the next line comes in.
    let ending = ((through - 0.88) / 0.12).clamp(0.0, 1.0);
    // In full screen the dots also jump on the beat.
    let kick = pulse.map_or(0.0, |pulse| 0.22 * pulse.beat + 0.1 * pulse.bass);
    let scale = lit * (breathe + kick) * (1.0 + 0.15 * ending - 1.15 * ending * ending);
    let center_y = rect.center().y;
    for dot in 0..3 {
        let fill = (through * 3.0 - dot as f32).clamp(0.0, 1.0);
        let center = pos2(rect.left() + radius + 2.0 + gap * dot as f32, center_y);
        let alpha = (0.35 + 0.65 * fill) * lit;
        let r = radius * scale.max(0.0);
        if let Some(pulse) = pulse
            && fill > 0.0
        {
            lyrics_fx::paint_blob(
                ui.painter(),
                center,
                egui::Vec2::splat(r * 4.5),
                light(pulse.accent, (0.14 + 0.2 * pulse.beat) * fill * lit),
            );
        }
        if fill > 0.0 {
            ui.painter()
                .circle_filled(center, r * 1.6, color.gamma_multiply(0.08 * fill * lit));
        }
        ui.painter()
            .circle_filled(center, r, color.gamma_multiply(alpha));
    }
}

/// The blurred cover, drawn as a few slowly turning layers so the colours
/// drift behind the words. They turn faster while the song is loud, and
/// brighter copies sweep across on top as light.
fn dynamic_backdrop(
    painter: &egui::Painter,
    rect: Rect,
    texture: &egui::TextureHandle,
    fx: &lyrics_fx::Fx,
    time: f32,
) {
    painter.image(
        texture.id(),
        rect,
        cover_uv(rect.size(), texture.size_vec2()),
        Color32::from_gray(190),
    );
    let reach = rect.width().max(rect.height());
    // Big soft discs of the cover drifting and turning at different
    // speeds, the brightest in the middle, so its colours flow slowly.
    let layers = [
        (vec2(0.5, 0.5), 0.95, -0.035, 0.0, 190),
        (vec2(0.15, 0.2), 0.7, 0.06, 1.7, 225),
        (vec2(0.88, 0.82), 0.68, -0.075, 3.4, 215),
        (vec2(0.78, 0.18), 0.5, 0.09, 5.1, 205),
        (vec2(0.35, 0.62), 0.55, -0.05, 2.3, 235),
    ];
    for (at, radius, speed, phase, gray) in layers {
        let drift = vec2(
            (time * 0.045 + phase).sin() * 0.09,
            (time * 0.035 + phase * 1.3).cos() * 0.07,
        );
        let breathe = 1.0 + 0.06 * (time * 0.11 + phase).sin() + 0.035 * fx.bass;
        let center = rect.min + (at + drift) * rect.size();
        let angle = (time + fx.flow * 1.5) * speed + phase;
        let gray = (gray as f32 + 12.0 * fx.beat).min(255.0) as u8;
        paint_disc(
            painter,
            texture.id(),
            center,
            reach * radius * breathe,
            angle,
            Color32::from_gray(gray),
        );
    }
    // The same colours again as light, circling faster while it is loud.
    let lights = [
        (0.0, 0.33, 0.21, 0.4),
        (2.1, -0.27, 0.3, 0.34),
        (4.2, 0.19, -0.26, 0.3),
    ];
    let strength = 0.08 + 0.12 * fx.level + 0.03 * fx.beat;
    for (phase, turn, orbit, radius) in lights {
        let around = fx.flow * orbit + phase;
        let center = rect.center()
            + vec2(
                around.cos() * rect.width() * 0.34,
                (around * 1.3).sin() * rect.height() * 0.3,
            );
        paint_disc(
            painter,
            texture.id(),
            center,
            reach * radius * (1.0 + 0.08 * fx.bass),
            fx.flow * turn + phase,
            light(Color32::WHITE, strength),
        );
    }
}

/// A disc of `texture` turned by `angle`, solid in the middle and fading
/// out at its rim.
fn paint_disc(
    painter: &egui::Painter,
    texture: egui::TextureId,
    center: egui::Pos2,
    radius: f32,
    angle: f32,
    tint: Color32,
) {
    const SEGMENTS: u32 = 48;
    const SOLID: f32 = 0.55;
    let (sin, cos) = angle.sin_cos();
    // The rim maps inside the texture at any turn.
    let uv = |dx: f32, dy: f32| {
        let x = dx * cos - dy * sin;
        let y = dx * sin + dy * cos;
        pos2(0.5 + x * 0.35, 0.5 + y * 0.35)
    };
    let mut mesh = egui::Mesh::with_texture(texture);
    mesh.vertices.push(egui::epaint::Vertex {
        pos: center,
        uv: uv(0.0, 0.0),
        color: tint,
    });
    for segment in 0..SEGMENTS {
        let theta = segment as f32 * std::f32::consts::TAU / SEGMENTS as f32;
        let (dy, dx) = theta.sin_cos();
        for (ring, color) in [(SOLID, tint), (1.0, Color32::TRANSPARENT)] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: center + vec2(dx, dy) * radius * ring,
                uv: uv(dx * ring, dy * ring),
                color,
            });
        }
    }
    for segment in 0..SEGMENTS {
        let inner = 1 + segment * 2;
        let outer = inner + 1;
        let next_inner = 1 + ((segment + 1) % SEGMENTS) * 2;
        let next_outer = next_inner + 1;
        mesh.add_triangle(0, inner, next_inner);
        mesh.add_triangle(inner, outer, next_outer);
        mesh.add_triangle(inner, next_outer, next_inner);
    }
    painter.add(mesh);
}

/// How a list of lyrics is drawn in one of the two views.
struct LinesStyle {
    color: Color32,
    size: f32,
    gap: f32,
    id: egui::Id,
    light_up: f32,
    /// Scroll the line being sung into place.
    follow: bool,
    animation: Option<egui::style::ScrollAnimation>,
    /// Lines fade out at the edges of this rect, when given.
    viewport: Option<Rect>,
    /// Let keyboard scrolling step through the rows.
    autoscroll_rows: bool,
    /// The song's pulse, for the full screen's larger gestures.
    glow: Option<Glow>,
}

struct LinesDrawn {
    /// A line was clicked: play from its start.
    seek: Option<u32>,
    /// Something is moving and wants the next frame.
    animating: bool,
}

/// Every line of `lyrics`, lit by where `now` is in the song.
fn lines(
    ui: &mut egui::Ui,
    lyrics: &crate::lyrics::Lyrics,
    now: &crate::app::NowPlaying,
    style: LinesStyle,
) -> LinesDrawn {
    let mut drawn = LinesDrawn {
        seek: None,
        animating: false,
    };
    let active = lyrics.active_line(now.position_ms);
    let font = theme::lyrics(style.size);
    // A long wait before the first line shows the dots at the top.
    if lyrics.synced
        && let Some(first) = lyrics.lines.first().and_then(|line| line.at_ms)
        && first >= INTRO_DOTS_MS
    {
        let current = active.is_none();
        let lit = ui
            .ctx()
            .animate_bool_with_time(style.id.with("intro"), current, style.light_up);
        interlude_row(
            ui,
            style.color,
            style.size,
            lit,
            gap_fraction(lyrics, None, now.position_ms),
            style.glow,
        );
        if lit > 0.0 {
            ui.add_space(style.gap * lit);
        }
        drawn.animating |= current && now.playing || (lit > 0.0 && lit < 1.0);
    }
    for (index, line) in lyrics.lines.iter().enumerate() {
        let is_active = active == Some(index);
        let lit = ui
            .ctx()
            .animate_bool_with_time(style.id.with(index), is_active, style.light_up);
        let distance = match active {
            Some(active) => active.abs_diff(index),
            None => index + 1,
        };
        drawn.animating |= lit > 0.0 && lit < 1.0;
        // A timed line with no words is the band playing on.
        if line.text.trim().is_empty() && lyrics.synced {
            let through = gap_fraction(lyrics, Some(index), now.position_ms);
            let top = ui.cursor().top();
            interlude_row(ui, style.color, style.size, lit, through, style.glow);
            if lit > 0.0 {
                ui.add_space(style.gap * lit);
            }
            drawn.animating |= is_active && now.playing;
            if is_active && style.follow {
                let rect = Rect::from_min_max(
                    pos2(ui.min_rect().left(), top),
                    pos2(ui.min_rect().right(), top + style.size),
                );
                show_sung_line(ui, rect, style.animation);
            }
            continue;
        }
        let words = if lyrics.synced && !crate::bidi::is_rtl(&line.text) {
            timed_words(lyrics, index)
        } else {
            Vec::new()
        };
        let sung = if !lyrics.synced || active.is_none_or(|active| index > active) {
            0.0
        } else if index < active.unwrap_or(index) {
            1.0
        } else if let Some(last) = words.last() {
            let first = words.first().map_or(last.start, |word| word.start);
            (now.position_ms.saturating_sub(first) as f32
                / last.end.saturating_sub(first).max(1) as f32)
                .clamp(0.0, 0.999)
        } else {
            sung_fraction(lyrics, index, now.position_ms)
        };
        drawn.animating |= is_active
            && now.playing
            && match words.last() {
                Some(last) => now.position_ms < last.end + 600,
                None => sung < 1.0,
            };
        let look = Look {
            color: style.color,
            lit: if lyrics.synced { lit } else { 1.0 },
            sung: if lyrics.synced { sung } else { 1.0 },
            distance: if lyrics.synced { distance } else { 0 },
            glow: style.glow.filter(|_| lyrics.synced),
        };
        let sense = if lyrics.synced {
            Sense::click()
        } else {
            Sense::hover()
        };
        let edge = style.viewport.map_or(1.0, |viewport| {
            let center = ui.cursor().top() + style.size * 0.6;
            let edge = ((center - viewport.top()).min(viewport.bottom() - center)
                / (style.size * 2.0))
                .clamp(0.0, 1.0);
            edge * edge * (3.0 - 2.0 * edge)
        });
        let response = ui
            .scope(|ui| {
                ui.multiply_opacity(edge);
                if words.is_empty() {
                    lyric_row(ui, &line.text, font.clone(), look, sense)
                } else {
                    word_row(ui, &words, font.clone(), look, now.position_ms, sense)
                }
            })
            .inner;
        if style.autoscroll_rows {
            crate::autoscroll::row(ui, &response);
        }
        if lyrics.synced
            && response.clicked()
            && let Some(at_ms) = line.at_ms
        {
            drawn.seek = Some(at_ms);
        }
        if lyrics.synced && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if is_active && style.follow {
            show_sung_line(ui, response.rect, style.animation);
        }
        ui.add_space(style.gap);
    }
    drawn
}

pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let fit = super::yielding_panel(
        ui.ctx(),
        "lyrics-panel",
        theme::SIDE_PANEL_MIN_WIDTH..=640.0,
        app.settings.lyrics_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let panel = egui::Panel::right("lyrics-panel")
        .resizable(true)
        .default_size(app.settings.lyrics_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.panel)
                .inner_margin(Margin::symmetric(12, 12)),
        );
    let response = panel.show(ui, |ui| {
        let window_controls = super::window_controls_reservation(
            ui.ctx(),
            app.show_queue_panel,
            app.show_lyrics_panel,
            ui.available_width(),
        );
        ui.add_space(window_controls.lyrics_top);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            theme::text(
                ui,
                gettext(app.locale, "Lyrics"),
                theme::bold(18.0),
                palette.text,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::icon_button(
                    ui,
                    Icon::X,
                    18.0,
                    palette.secondary,
                    palette.text,
                    &gettext(app.locale, "Close"),
                )
                .clicked()
                {
                    app.actions.push(Action::ToggleLyricsPanel);
                }
                if theme::icon_button(
                    ui,
                    Icon::Expand,
                    18.0,
                    palette.secondary,
                    palette.text,
                    &gettext(app.locale, "Full screen lyrics"),
                )
                .clicked()
                {
                    app.actions.push(Action::SetLyricsFullscreen(true));
                }
                let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
                if loaded
                    && !app.lyrics_following
                    && theme::pill_button(
                        ui,
                        &palette,
                        &pgettext(app.locale, "lyrics", "Follow"),
                        false,
                    )
                    .clicked()
                {
                    app.lyrics_following = true;
                    app.lyrics_line_shown = None;
                }
            });
        });
        ui.add_space(8.0);
        contents(app, ui);
    });
    let current_width = response.response.rect.width();
    if (app.settings.lyrics_width - current_width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), "lyrics-panel", &fit)
    {
        app.settings.lyrics_width = current_width;
        app.actions.push(Action::SettingsChanged);
    }
}

fn contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            widgets::loading_row(ui, &palette, app.locale);
            return;
        }
        Loadable::Failed(error) => {
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.secondary);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.request_lyrics();
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics.clone(),
    };

    let active = lyrics.active_line(now.position_ms);
    let follow = app.lyrics_following && app.lyrics_line_shown != Some(active);
    let mut animating = false;
    let scroll = crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("lyrics-scroll")
            .auto_shrink([false, false]),
        egui::Vec2b::new(false, true),
        |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                );
            }
            ui.add_space(12.0);
            let drawn = lines(
                ui,
                &lyrics,
                &now,
                LinesStyle {
                    color: palette.text,
                    size: LINE_SIZE,
                    gap: LINE_GAP,
                    id: egui::Id::new("lyric-line"),
                    light_up: LIGHT_UP_SECONDS,
                    follow,
                    animation: None,
                    viewport: None,
                    autoscroll_rows: true,
                    glow: None,
                },
            );
            animating = drawn.animating;
            if let Some(at_ms) = drawn.seek {
                app.actions.push(Action::Seek(at_ms));
                app.lyrics_following = true;
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if app.lyrics_following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                );
            }
            // Room for the last line to rise to where a sung line sits.
            ui.add_space((ui.clip_rect().height() * (1.0 - SUNG_LINE_AT)).max(60.0));
        },
    );
    crate::autoscroll::lyrics(ui, scroll.id);
    if animating {
        ui.ctx().request_repaint();
    }
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if ui.rect_contains_pointer(scroll.inner_rect)
        && ui.input(|input| input.smooth_scroll_delta.y != 0.0)
    {
        app.lyrics_following = false;
    }
    app.lyrics_line_shown = Some(active);
}

/// How much of the full screen controls show: all of them while the
/// pointer moves, fading away once it rests so only the cover and the
/// words remain, with the pointer hidden too.
fn chrome_opacity(ctx: &egui::Context) -> f32 {
    let resting = ctx.input(|input| input.pointer.time_since_last_movement()) > CHROME_REST_SECONDS
        && !ctx.input(|input| input.pointer.any_down());
    ctx.animate_bool_with_time(egui::Id::new("fullscreen-chrome"), !resting, 0.5)
}

/// Seconds the pointer rests before the full screen controls fade.
const CHROME_REST_SECONDS: f32 = 2.5;

pub fn fullscreen(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(theme::Palette::dark().window))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            background(app, ui, rect);
            let chrome = chrome_opacity(ui.ctx());
            if chrome < 0.01 {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            } else if chrome < 1.0 {
                ui.ctx().request_repaint();
            } else {
                // Look again when the pointer has rested long enough.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f32(CHROME_REST_SECONDS));
            }
            let top = theme::titlebar_inset(ui.ctx()) + 24.0;
            if app.now_playing().is_some() && rect.width() >= COVER_BESIDE_MIN_WIDTH {
                with_cover(app, ui, rect, top);
                return;
            }
            let width = fullscreen_content_width(rect.width());
            let region = Rect::from_min_max(
                pos2(rect.center().x - width / 2.0, rect.top() + top),
                pos2(rect.center().x + width / 2.0, rect.bottom()),
            );
            let mut content = ui.new_child(UiBuilder::new().max_rect(region));
            fullscreen_header(app, &mut content);
            content.add_space(20.0);
            track_heading(app, &mut content);
            content.add_space(16.0);
            fullscreen_contents(app, &mut content);
        });
}

/// The widest the lyrics get beside the cover, so lines stay easy to read.
const LYRICS_BESIDE_WIDTH: f32 = 720.0;

/// Room under the big cover for the title, artists and seek bar.
const COVER_CAPTION: f32 = 196.0;

/// The narrowest window that shows the cover beside the lyrics; narrower
/// ones keep a single column with a small cover in the heading.
const COVER_BESIDE_MIN_WIDTH: f32 = 900.0;

/// Full screen with the cover large: beside the lyrics when there are
/// words to follow, and alone in the middle when there are none, a calm
/// view of what is playing.
fn with_cover(app: &mut App, ui: &mut egui::Ui, rect: Rect, top: f32) {
    let outer = Rect::from_min_max(
        pos2(rect.left() + 48.0, rect.top() + top),
        pos2(rect.right() - 48.0, rect.bottom() - 40.0),
    );
    let mut header = ui.new_child(UiBuilder::new().max_rect(outer));
    fullscreen_header(app, &mut header);
    let below = Rect::from_min_max(
        pos2(outer.left(), header.min_rect().bottom() + 24.0),
        outer.max,
    );
    // The cover moves aside only for words to read. While they load it
    // stays in the middle, so a song that turns out to have none never
    // moves at all.
    let words = matches!(&app.lyrics, Loadable::Loaded(Some(lyrics)) if !lyrics.instrumental);
    if words {
        // The cover and the lyrics are one group, centred in the window.
        let gap = 64.0;
        let side = (below.width() * 0.38)
            .min(below.height() - COVER_CAPTION)
            .clamp(200.0, 520.0);
        let lyrics_width = (below.width() - side - gap).min(LYRICS_BESIDE_WIDTH);
        let left = below.center().x - (side + gap + lyrics_width) / 2.0;
        let column = Rect::from_min_size(
            pos2(left, below.center().y - (side + COVER_CAPTION) / 2.0),
            vec2(side, side + COVER_CAPTION),
        );
        big_cover(app, ui, column, Align::Min);
        let lyrics = Rect::from_min_max(
            pos2(column.right() + gap, below.top()),
            pos2(column.right() + gap + lyrics_width, below.bottom()),
        );
        let mut content = ui.new_child(UiBuilder::new().max_rect(lyrics));
        fullscreen_contents(app, &mut content);
    } else {
        let side = (below.height() - COVER_CAPTION - 50.0)
            .min(below.width() * 0.5)
            .clamp(200.0, 560.0);
        let column = Rect::from_center_size(below.center(), vec2(side, side + COVER_CAPTION));
        big_cover(app, ui, column, Align::Center);
        // Why there are no words, quietly, under the song, or that they
        // are still being fetched.
        let (heading, detail) = match &app.lyrics {
            Loadable::Loaded(Some(_)) => (
                gettext(app.locale, "Instrumental"),
                gettext(app.locale, "No timed lyrics for this track."),
            ),
            Loadable::Loaded(None) => (
                gettext(app.locale, "No lyrics"),
                gettext(app.locale, "No lyrics found for this track."),
            ),
            Loadable::Failed(error) => (
                gettext(
                    app.locale,
                    // Translators: Keep {error} unchanged. It is the original failure detail.
                    "Couldn't fetch the lyrics: {error}",
                )
                .replace("{error}", error)
                .into(),
                Default::default(),
            ),
            Loadable::NotLoaded | Loadable::Loading => {
                (gettext(app.locale, "Loadingâ€¦"), Default::default())
            }
        };
        let heading = ui.painter().text(
            pos2(column.center().x, column.bottom() + 8.0),
            egui::Align2::CENTER_TOP,
            heading,
            theme::semibold(13.0),
            Color32::from_gray(200),
        );
        if matches!(app.lyrics, Loadable::Failed(_)) {
            let retry = Rect::from_center_size(
                pos2(column.center().x, heading.bottom() + 24.0),
                vec2(column.width(), 32.0),
            );
            let mut retry_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(retry)
                    .layout(Layout::top_down(Align::Center)),
            );
            let label = gettext(app.locale, "Try again");
            if theme::pill_button(&mut retry_ui, &theme::Palette::dark(), &label, false).clicked() {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        ui.painter().text(
            pos2(column.center().x, heading.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            detail,
            theme::regular(13.0),
            Color32::from_gray(170),
        );
    }
}

/// The playing song's cover filling the top of `column`, with its title,
/// artists and a seek bar beneath, aligned to its left edge or centred.
fn big_cover(app: &mut App, ui: &mut egui::Ui, column: Rect, align: Align) {
    let Some(now) = app.now_playing() else {
        return;
    };
    let side = column.width();
    let cover = Rect::from_min_size(column.min, vec2(side, side));
    let radius = 14.0;
    // The cover's own colour glows softly behind it.
    let (low, _) = app.lyrics_fx.colors;
    lyrics_fx::paint_blob(
        ui.painter(),
        cover.center(),
        egui::Vec2::splat(side * 0.9),
        light(low, 0.1),
    );
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 24],
            blur: 64,
            spread: 0,
            color: Color32::from_black_alpha(150),
        }
        .as_shape(cover, radius),
    );
    widgets::paint_cover(
        ui,
        &theme::Palette::dark(),
        now.art_url.as_deref().or(now.art_small.as_deref()),
        cover,
        radius,
        Icon::Music,
        Some(app.backend.art()),
    );
    let words = Rect::from_min_max(pos2(column.left(), cover.bottom() + 20.0), column.max);
    let mut text = ui.new_child(
        UiBuilder::new()
            .max_rect(words)
            .layout(Layout::top_down(align)),
    );
    text.spacing_mut().item_spacing.y = 4.0;
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.title)
                .font(theme::lyrics(24.0))
                .color(Color32::WHITE),
        )
        .truncate(),
    );
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.subtitle)
                .font(theme::medium(15.0))
                .color(Color32::from_white_alpha(170)),
        )
        .truncate(),
    );
    text.add_space(12.0);
    let chrome = chrome_opacity(ui.ctx());
    text.scope(|ui| {
        ui.multiply_opacity(chrome);
        seek_bar(app, ui, &now, side);
        ui.add_space(26.0);
        playback_buttons(app, ui, &now, side);
    });
}

/// Shuffle, previous, play or pause, next and repeat, centred under the seek
/// bar, since full screen has no player bar. The toggles stay quiet until
/// they are on, when they turn white with a small dot beneath, so the row
/// reads as one calm cluster over the backdrop.
fn playback_buttons(app: &mut App, ui: &mut egui::Ui, now: &crate::app::NowPlaying, width: f32) {
    const ROW: f32 = 56.0;
    const DISC: f32 = 56.0;
    // Icon buttons occupy icon size + 12.
    let widths = [30.0, 36.0, DISC, 36.0, 30.0];
    let gap = 22.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len() - 1) as f32;
    let top = ui.cursor().top();
    let cy = top + ROW / 2.0;
    let mut x = ui.cursor().left() + (width - total) / 2.0;
    let slots = widths.map(|w| {
        let rect = Rect::from_center_size(pos2(x + w / 2.0, cy), vec2(w, ROW));
        x += w + gap;
        rect
    });
    let cell = |ui: &mut egui::Ui, rect: Rect| {
        ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        )
    };
    let quiet = Color32::from_white_alpha(150);
    let soft = Color32::from_white_alpha(215);
    let on_dot = |ui: &egui::Ui, rect: Rect| {
        ui.painter()
            .circle_filled(pos2(rect.center().x, cy + 17.0), 2.0, Color32::WHITE);
    };

    let shuffle = now.shuffle;
    let mut c = cell(ui, slots[0]);
    let response = theme::icon_button(
        &mut c,
        Icon::Shuffle,
        18.0,
        if shuffle { Color32::WHITE } else { quiet },
        Color32::WHITE,
        &gettext(app.locale, "Shuffle"),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            c.is_enabled(),
            shuffle,
            gettext(app.locale, "Shuffle"),
        )
    });
    if shuffle {
        on_dot(ui, slots[0]);
    }
    if response.clicked() {
        app.actions.push(Action::ToggleShuffle);
    }

    let mut c = cell(ui, slots[1]);
    if theme::icon_button(
        &mut c,
        Icon::SkipBackFilled,
        24.0,
        soft,
        Color32::WHITE,
        &gettext(app.locale, "Previous"),
    )
    .clicked()
    {
        app.actions.push(Action::Previous);
    }

    let (icon, label) = if now.playing {
        (Icon::PauseFilled, gettext(app.locale, "Pause"))
    } else {
        (Icon::PlayFilled, gettext(app.locale, "Play"))
    };
    let disc = slots[2];
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(90),
        }
        .as_shape(
            Rect::from_center_size(disc.center(), egui::Vec2::splat(DISC)),
            DISC / 2.0,
        ),
    );
    let mut c = cell(ui, disc);
    if theme::circle_button(
        &mut c,
        icon,
        DISC,
        Color32::from_white_alpha(240),
        Color32::WHITE,
        Color32::from_gray(18),
        &label,
    )
    .clicked()
    {
        app.actions.push(Action::TogglePlay);
    }

    let mut c = cell(ui, slots[3]);
    if theme::icon_button(
        &mut c,
        Icon::SkipForwardFilled,
        24.0,
        soft,
        Color32::WHITE,
        &gettext(app.locale, "Next"),
    )
    .clicked()
    {
        app.actions.push(Action::Next);
    }

    let (icon, on, tooltip) = match now.repeat {
        RepeatMode::Off => (Icon::Repeat, false, gettext(app.locale, "Repeat")),
        RepeatMode::Context => (Icon::Repeat, true, gettext(app.locale, "Repeat one")),
        RepeatMode::Track => (Icon::Repeat1, true, gettext(app.locale, "Repeat off")),
    };
    let repeat = slots[4];
    let mut c = cell(ui, repeat);
    if theme::icon_button(
        &mut c,
        icon,
        18.0,
        if on { Color32::WHITE } else { quiet },
        Color32::WHITE,
        &tooltip,
    )
    .clicked()
    {
        app.actions.push(Action::CycleRepeat);
    }
    if on {
        on_dot(ui, repeat);
    }
    ui.advance_cursor_after_rect(Rect::from_min_size(
        pos2(ui.cursor().left(), top),
        vec2(width, ROW),
    ));
}

/// A white seek bar with the elapsed and total time under it.
fn seek_bar(app: &mut App, ui: &mut egui::Ui, now: &crate::app::NowPlaying, width: f32) {
    let mut palette = theme::Palette::dark();
    palette.accent = Color32::WHITE;
    palette.text = Color32::from_white_alpha(220);
    let duration = now.duration_ms;
    let fraction = if duration > 0 {
        now.position_ms as f32 / duration as f32
    } else {
        0.0
    };
    let mut bar = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(ui.cursor().min, vec2(width, 16.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    match widgets::thin_slider(
        &mut bar,
        &palette,
        egui::Id::new("fullscreen-seek-slider"),
        &gettext(app.locale, "Playback position (%)"),
        fraction,
        width,
        None,
    ) {
        widgets::SliderEvent::Dragging(value) => app.seek_preview = Some(value),
        widgets::SliderEvent::Committed(value) => {
            app.seek_preview = None;
            if duration > 0 {
                app.actions
                    .push(Action::Seek((value * duration as f32) as u32));
            }
        }
        widgets::SliderEvent::None => {}
    }
    let shown = match app.seek_preview {
        Some(fraction) => (fraction * duration as f32) as u32,
        None => now.position_ms,
    };
    ui.add_space(20.0);
    let row = ui.cursor().min;
    let color = Color32::from_white_alpha(150);
    ui.painter().text(
        row,
        egui::Align2::LEFT_TOP,
        crate::util::format_duration_ms(shown),
        theme::medium(12.0),
        color,
    );
    ui.painter().text(
        pos2(row.x + width, row.y),
        egui::Align2::RIGHT_TOP,
        crate::util::format_duration_ms(duration),
        theme::medium(12.0),
        color,
    );
}

fn fullscreen_content_width(viewport_width: f32) -> f32 {
    let available = (viewport_width - 48.0).max(0.0);
    (viewport_width * 0.72).clamp(400.0, 960.0).min(available)
}

fn preferred_backdrop_art(small: Option<String>, large: Option<String>) -> Option<String> {
    small.or(large)
}

fn background(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    theme::apply_local(ui, &theme::Palette::dark());
    let now = app.now_playing();
    let (sounding, playing) = now.as_ref().map_or((false, false), |now| {
        ((now.playing || now.loading) && now.local, now.playing)
    });
    let art = now.and_then(|now| preferred_backdrop_art(now.art_small, now.art_url));
    let painter = ui.painter().with_clip_rect(rect);
    let clock = ui.input(|input| input.time);
    let time = clock as f32;
    app.lyrics_fx
        .update(&app.winamp.tap, sounding, playing, clock);
    app.lyrics_fx.colors = lyrics_fx::lights(app.tint_for(art.as_deref()));
    let (low, high) = app.lyrics_fx.colors;
    if let Some(texture) = app
        .lyrics_backdrop
        .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        dynamic_backdrop(&painter, rect, texture, &app.lyrics_fx, time);
    }
    let fx = &app.lyrics_fx;
    if playing || fx.moving() {
        // Everything moves with the music: every frame counts.
        ui.ctx().request_repaint();
    } else {
        // Paused, the layers still turn slowly; a few frames a second
        // keep them smooth.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(72));
    fx.wave(&painter, rect, low, high);
    fx.embers(&painter, rect, low, time);
    lyrics_fx::vignette(&painter, rect, 0.5);
    widgets::paint_vertical_gradient(
        ui,
        rect,
        Color32::from_black_alpha(0),
        Color32::from_black_alpha(95),
    );
}

fn cover_uv(view: egui::Vec2, image: egui::Vec2) -> Rect {
    let ratio = (view.x / view.y.max(1.0)) / (image.x / image.y.max(1.0));
    let size = if ratio > 1.0 {
        vec2(1.0, 1.0 / ratio)
    } else {
        vec2(ratio, 1.0)
    };
    Rect::from_center_size(pos2(0.5, 0.5), size)
}

fn fullscreen_header(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    // No title: the cover and the words say what this is. The buttons fade
    // with the other controls when the pointer rests.
    ui.multiply_opacity(chrome_opacity(ui.ctx()));
    ui.horizontal(|ui| {
        ui.set_min_height(26.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::Shrink,
                18.0,
                palette.text,
                palette.text,
                &gettext(app.locale, "Leave full screen (Esc)"),
            )
            .clicked()
            {
                app.actions.push(Action::SetLyricsFullscreen(false));
            }
            let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
            if loaded
                && !app.lyrics_following
                && theme::pill_button(
                    ui,
                    &palette,
                    &pgettext(app.locale, "lyrics", "Follow"),
                    false,
                )
                .clicked()
            {
                app.actions.push(Action::FollowLyrics);
            }
        });
    });
}

fn track_heading(app: &App, ui: &mut egui::Ui) {
    if let Some(now) = app.now_playing() {
        ui.horizontal(|ui| {
            let size = 52.0;
            let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
            widgets::paint_cover(
                ui,
                &theme::Palette::dark(),
                now.art_small.as_deref().or(now.art_url.as_deref()),
                rect,
                4.0,
                Icon::Music,
                Some(app.backend.art()),
            );
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&now.title)
                            .font(theme::semibold(22.0))
                            .color(Color32::WHITE),
                    )
                    .truncate(),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&now.subtitle)
                            .font(theme::regular(13.0))
                            .color(Color32::from_gray(235)),
                    )
                    .truncate(),
                );
            });
        });
    }
}

fn fullscreen_contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            widgets::loading_row(ui, &palette, app.locale);
            return;
        }
        Loadable::Failed(error) => {
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.text);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics.clone(),
    };

    let active = lyrics.active_line(now.position_ms);
    // A Follow click resets the remembered line after drawing. Other frames
    // record the shown line before any line-click action restores following.
    if !app
        .actions
        .iter()
        .any(|action| matches!(action, Action::FollowLyrics))
    {
        app.actions.push(Action::LyricsLineShown(active));
    }
    let viewport = ui.available_rect_before_wrap();
    let manual_scroll = ui.rect_contains_pointer(viewport)
        && ui.input(|input| {
            input.smooth_scroll_delta.y != 0.0
                || (input.pointer.primary_down() && input.pointer.delta().y != 0.0)
        });
    let following = app.lyrics_following && !manual_scroll;
    let follow = following && app.lyrics_line_shown != Some(active);
    let animation = egui::style::ScrollAnimation::duration(0.45);
    let size = (ui.available_width() * 0.06).clamp(30.0, 50.0);
    let mut animating = false;
    ui.spacing_mut().scroll.fade.strength = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("fullscreen-lyrics-scroll", &now.uri))
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                    animation,
                );
            }
            // Room for the first line to sit where a sung line sits, and for
            // the last to rise to it.
            let (padding, below) = if lyrics.synced {
                (
                    (viewport.height() * SUNG_LINE_AT - size).max(12.0),
                    viewport.height() * (1.0 - SUNG_LINE_AT),
                )
            } else {
                (12.0, 60.0)
            };
            ui.add_space(padding);
            let drawn = lines(
                ui,
                &lyrics,
                &now,
                LinesStyle {
                    color: palette.text,
                    size,
                    gap: size * 0.62,
                    id: egui::Id::new("lyric-line").with(("fullscreen", &now.uri)),
                    light_up: 0.3,
                    follow,
                    animation: Some(animation),
                    viewport: Some(viewport),
                    autoscroll_rows: false,
                    glow: Some(app.lyrics_fx.glow(app.lyrics_fx.colors.0)),
                },
            );
            animating = drawn.animating;
            if let Some(at_ms) = drawn.seek {
                app.actions.push(Action::Seek(at_ms));
                app.actions.push(Action::FollowLyrics);
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                    animation,
                );
            }
            ui.add_space(below.max(60.0));
        });
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if manual_scroll && app.lyrics_following {
        app.actions.push(Action::PauseLyricsFollow);
    }
    if animating {
        ui.ctx().request_repaint();
    }
    if now.playing
        && lyrics.synced
        && let Some(next) = lyrics
            .lines
            .iter()
            .filter_map(|line| line.at_ms)
            .find(|at| *at > now.position_ms)
    {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(u64::from(
                next - now.position_ms,
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fullscreen_content_width, gap_fraction, glow, idle_opacity, preferred_backdrop_art,
        sung_fraction, timed_words,
    };
    use crate::lyrics::{Line, Lyrics};

    fn timed(lines: &[(u32, &str)]) -> Lyrics {
        Lyrics {
            lines: lines
                .iter()
                .map(|(at_ms, text)| Line {
                    at_ms: Some(*at_ms),
                    text: (*text).to_string(),
                    words: Vec::new(),
                })
                .collect(),
            synced: true,
            instrumental: false,
        }
    }

    /// The sweep runs through a line in time with its words, finishes
    /// before the next line even when the gap is short, and does not drag
    /// across a long pause.
    #[test]
    fn a_sung_line_sweeps_in_time_and_never_past_the_next_line() {
        let lyrics = timed(&[
            (1_000, "Short"),
            (2_000, "A rather longer line of words"),
            (60_000, ""),
        ]);
        assert_eq!(sung_fraction(&lyrics, 0, 900), 0.0);
        assert!((sung_fraction(&lyrics, 0, 1_500) - 0.5).abs() < 1e-3);
        assert_eq!(sung_fraction(&lyrics, 0, 2_000), 1.0);
        // 29 characters at 80 ms each, not the 58 seconds before the next line.
        let halfway = 2_000 + 29 * 80 / 2;
        assert!((sung_fraction(&lyrics, 1, halfway) - 0.5).abs() < 1e-3);
        assert_eq!(sung_fraction(&lyrics, 1, 10_000), 1.0);
    }

    /// The dots fill across the gap they stand for: the intro before the
    /// first line, or an empty line until the next one.
    #[test]
    fn interlude_dots_follow_the_gap() {
        let lyrics = timed(&[(4_000, "First"), (6_000, ""), (10_000, "Back")]);
        assert!((gap_fraction(&lyrics, None, 2_000) - 0.5).abs() < 1e-3);
        assert_eq!(gap_fraction(&lyrics, Some(1), 6_000), 0.0);
        assert!((gap_fraction(&lyrics, Some(1), 9_000) - 0.75).abs() < 1e-3);
        assert_eq!(gap_fraction(&lyrics, Some(1), 12_000), 1.0);
    }

    /// Lines timed only by line still light word by word, in order and
    /// within the line's sweep; word stamps are used as they are.
    #[test]
    fn every_timed_line_is_sung_word_by_word() {
        let lyrics = timed(&[(1_000, "Every window holding"), (9_000, "")]);
        let words = timed_words(&lyrics, 0);
        let texts: Vec<&str> = words.iter().map(|word| word.text).collect();
        assert_eq!(texts, ["Every ", "window ", "holding"]);
        assert_eq!(words[0].start, 1_000);
        assert!(words.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert!(words[2].end <= 9_000);

        let mut stamped = timed(&[(1_000, "Hey you"), (5_000, "")]);
        stamped.lines[0].words = vec![
            crate::lyrics::Word {
                at_ms: 1_000,
                text: "Hey ".into(),
                end_ms: None,
            },
            crate::lyrics::Word {
                at_ms: 1_800,
                text: "you".into(),
                end_ms: None,
            },
        ];
        let words = timed_words(&stamped, 0);
        assert_eq!((words[0].start, words[0].end), (1_000, 1_800));
        assert_eq!(words[1].start, 1_800);
        assert!(words[1].end > 1_800 && words[1].end <= 5_000);
    }

    #[test]
    fn the_glow_rises_holds_and_lets_go() {
        assert_eq!(glow(0.0), 0.0);
        assert_eq!(glow(0.5), 1.0);
        assert_eq!(glow(0.9), 1.0);
        assert!(glow(1.0) < 1e-6);
        assert!(glow(0.25) > 0.0 && glow(0.25) < 1.0);
    }

    /// Lines fade with distance from the sung one, but never vanish.
    #[test]
    fn distant_lines_fade_to_a_floor() {
        assert_eq!(idle_opacity(1, false), idle_opacity(0, false));
        assert!(idle_opacity(3, false) < idle_opacity(1, false));
        assert!(idle_opacity(100, false) > 0.0);
        assert!(idle_opacity(100, true) > idle_opacity(1, false));
    }

    #[test]
    fn fullscreen_backdrop_prefers_small_art_with_large_art_as_fallback() {
        let small = "small".to_string();
        let large = "large".to_string();
        assert_eq!(
            preferred_backdrop_art(Some(small.clone()), Some(large.clone())),
            Some(small)
        );
        assert_eq!(
            preferred_backdrop_art(None, Some(large.clone())),
            Some(large)
        );
        assert_eq!(preferred_backdrop_art(None, None), None);
    }

    #[test]
    fn fullscreen_content_width_never_inverts_a_narrow_viewport() {
        for viewport_width in [0.0, 24.0, 47.0, 48.0, 64.0, 760.0, 2_000.0] {
            let width = fullscreen_content_width(viewport_width);
            assert!(width >= 0.0);
            assert!(width <= (viewport_width - 48.0).max(0.0));
        }
        assert_eq!(fullscreen_content_width(47.0), 0.0);
        assert_eq!(fullscreen_content_width(2_000.0), 960.0);
    }
}
