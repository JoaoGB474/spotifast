//! The light that moves with the music in full screen lyrics: how loud the
//! song is and where its beats fall, and the glows, embers and waves drawn
//! from them behind and around the words.

use egui::{Color32, Mesh, Pos2, Rect, Vec2, pos2, vec2};

use crate::vis;

/// Samples read for the loudness and the beat, about a fortieth of a second.
const PULSE_SAMPLES: usize = 1_024;
/// How much of each sample the bass filter lets through: a one-pole
/// low-pass near 150 Hz.
const BASS_PASS: f32 = 0.021;
/// How far over its recent average the bass must jump to count as a beat.
const BEAT_OVER: f32 = 1.38;
/// The shortest time between two beats, so one kick is one beat.
const BEAT_GAP: f32 = 0.16;
/// Rings of light kept at once, the oldest dropped first.
const RINGS: usize = 4;
/// How long a ring takes to spread out and fade.
const RING_SECONDS: f32 = 1.6;
/// Embers drifting up the screen.
const EMBERS: usize = 90;
/// Segments round a soft blob of light.
const BLOB_SEGMENTS: u32 = 20;

/// The song's pulse, read once a frame and shared by everything that moves
/// with it.
pub struct Fx {
    analyser: vis::WideAnalyser,
    /// The spectrum, smoothed across neighbouring bands and over time.
    pub bands: [f32; vis::WIDE_BANDS],
    last: Option<f64>,
    /// The bass's recent average, which a beat has to jump over.
    average: f32,
    since_beat: f32,
    /// How strong the bass is right now, from 0 to 1, quick up, slow down.
    pub bass: f32,
    /// How loud the whole song is right now, from 0 to 1.
    pub level: f32,
    /// 1 on a beat, falling away to 0 after it.
    pub beat: f32,
    /// Time that runs faster while the song is loud, for what drifts.
    pub flow: f32,
    /// The cover's colour as light, and its companion; see [`lights`].
    pub colors: (Color32, Color32),
    /// Seconds since each ring of light left the cover.
    rings: Vec<f32>,
    /// Whether any sound reached the tap lately; without it the pulse
    /// breathes on its own.
    heard: f32,
}

impl Default for Fx {
    fn default() -> Self {
        Self {
            analyser: vis::WideAnalyser::default(),
            bands: [0.0; vis::WIDE_BANDS],
            last: None,
            average: 0.0,
            since_beat: 1.0,
            bass: 0.0,
            level: 0.0,
            beat: 0.0,
            flow: 0.0,
            colors: lights(None),
            rings: Vec::new(),
            heard: 0.0,
        }
    }
}

/// What the words need of the pulse to glow in time.
#[derive(Clone, Copy)]
pub struct Glow {
    pub accent: Color32,
    pub bass: f32,
    pub beat: f32,
}

impl Fx {
    /// Reads the sound playing now. `sounding` is whether this computer is
    /// the one playing; a song on another device has no sound to read, so
    /// the pulse breathes slowly instead.
    pub fn update(&mut self, tap: &vis::AudioTap, sounding: bool, playing: bool, time: f64) {
        let dt = self
            .last
            .map_or(1.0 / 60.0, |last| (time - last) as f32)
            .clamp(0.0, 0.1);
        self.last = Some(time);
        let samples = if sounding {
            tap.window(PULSE_SAMPLES, vis::LAG)
        } else {
            Vec::new()
        };
        let (level, bass) = loudness(&samples);
        let heard = level > 0.002;
        self.heard = approach(self.heard, if heard { 1.0 } else { 0.0 }, dt * 2.0);

        let spectrum = self.analyser.step(
            &samples[samples.len().saturating_sub(vis::WIDE_SAMPLES)..],
            std::time::Instant::now(),
        );
        for band in 0..vis::WIDE_BANDS {
            let near = |offset: isize| {
                let index = (band as isize + offset).clamp(0, vis::WIDE_BANDS as isize - 1);
                spectrum[index as usize]
            };
            let smooth =
                (near(-2) + 2.0 * near(-1) + 3.0 * near(0) + 2.0 * near(1) + near(2)) / 9.0;
            self.bands[band] = follow(self.bands[band], smooth, dt, 30.0, 7.0);
        }

        // Loudness on a scale the eye reads: quiet passages still move.
        let level = (level * 3.2).sqrt().min(1.0);
        let bass = (bass * 4.5).sqrt().min(1.0);
        self.since_beat += dt;
        self.beat *= (-dt * 5.5).exp();
        if heard {
            if bass > self.average * BEAT_OVER && bass > 0.18 && self.since_beat > BEAT_GAP {
                self.beat = 1.0;
                self.since_beat = 0.0;
                if self.rings.len() == RINGS {
                    self.rings.remove(0);
                }
                self.rings.push(0.0);
            }
            self.average = approach(self.average, bass, dt * 2.2);
            self.bass = follow(self.bass, bass, dt, 40.0, 4.5);
            self.level = follow(self.level, level, dt, 20.0, 2.5);
        } else {
            // Nothing to hear: a slow breath while the song plays elsewhere,
            // and stillness while it is paused.
            let breath = if playing {
                0.22 + 0.12 * (time as f32 * 0.9).sin()
            } else {
                0.0
            };
            self.average = approach(self.average, 0.0, dt * 2.2);
            self.bass = follow(self.bass, breath, dt, 3.0, 3.0);
            self.level = follow(self.level, breath, dt, 3.0, 3.0);
        }
        self.flow += dt * (0.35 + 1.4 * self.level + 1.2 * self.beat);
        for age in &mut self.rings {
            *age += dt;
        }
        self.rings.retain(|age| *age < RING_SECONDS);
    }

    pub fn glow(&self, accent: Color32) -> Glow {
        Glow {
            accent,
            bass: self.bass,
            beat: self.beat,
        }
    }

    /// Whether anything is still moving and wants another frame.
    pub fn moving(&self) -> bool {
        self.level > 0.005 || self.beat > 0.005 || !self.rings.is_empty()
    }

    /// Rings of light spreading from `center` since the last few beats.
    pub fn rings(
        &self,
        painter: &egui::Painter,
        center: Pos2,
        start: f32,
        reach: f32,
        color: Color32,
    ) {
        for age in &self.rings {
            let through = age / RING_SECONDS;
            let eased = 1.0 - (1.0 - through).powi(3);
            let radius = start + (reach - start) * eased;
            let strength = (1.0 - through).powi(2) * 0.2;
            ring(
                painter,
                center,
                radius,
                (reach * 0.06).max(12.0) * (0.5 + through),
                light(color, strength),
            );
        }
    }

    /// Embers drifting up `rect`, faster and brighter while the song is
    /// loud, twinkling each at its own pace.
    pub fn embers(&self, painter: &egui::Painter, rect: Rect, accent: Color32, time: f32) {
        let mut mesh = Mesh::default();
        for ember in 0..EMBERS {
            let seed = |salt: u32| hash(ember as u32 * 7 + salt);
            let depth = seed(0);
            let speed = 0.012 + 0.03 * depth;
            let rise = (seed(1) - self.flow * speed).rem_euclid(1.0);
            let sway = (time * (0.2 + 0.4 * seed(2)) + seed(3) * std::f32::consts::TAU).sin();
            let x = seed(4) + sway * 0.018 * (0.5 + depth);
            // Fade in from the bottom edge and out before the top.
            let life = (rise * 6.0).min(1.0) * ((1.0 - rise) * 5.0).min(1.0);
            let twinkle = 0.55
                + 0.45 * (time * (0.7 + 2.2 * seed(5)) + seed(6) * std::f32::consts::TAU).sin();
            let strength =
                life * twinkle * (0.1 + 0.3 * self.level + 0.25 * self.beat) * (0.4 + depth);
            if strength < 0.004 {
                continue;
            }
            let center = pos2(
                rect.left() + rect.width() * x.rem_euclid(1.0),
                rect.bottom() - rect.height() * rise,
            );
            let radius = (1.5 + 7.0 * depth * depth) * (1.0 + 0.5 * self.beat);
            // Most embers take the cover's colour; a few stay white.
            let color = if seed(7) > 0.7 {
                Color32::WHITE
            } else {
                accent
            };
            blob(
                &mut mesh,
                center,
                Vec2::splat(radius),
                light(color, strength),
            );
        }
        painter.add(mesh);
    }

    /// The spectrum as a soft wave of light rising from the bottom edge.
    pub fn wave(&self, painter: &egui::Painter, rect: Rect, low: Color32, high: Color32) {
        let height = rect.height() * 0.2;
        let points = 96;
        let mut mesh = Mesh::default();
        for point in 0..=points {
            let across = point as f32 / points as f32;
            // Mirrored about the middle, the bass in the centre.
            let from_middle = (across - 0.5).abs() * 2.0;
            let level = self.band(from_middle * 0.8);
            let x = rect.left() + rect.width() * across;
            let top = rect.bottom() - height * (0.04 + level);
            let color = mix(low, high, from_middle);
            let strength = 0.1 + 0.34 * level;
            let index = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x, rect.bottom()), light(color, strength));
            mesh.colored_vertex(pos2(x, top), Color32::TRANSPARENT);
            if point > 0 {
                mesh.add_triangle(index - 2, index - 1, index);
                mesh.add_triangle(index - 1, index, index + 1);
            }
        }
        painter.add(mesh);
    }

    /// The smoothed spectrum at `at`, from 0 (the lowest band) to 1.
    fn band(&self, at: f32) -> f32 {
        let position = at.clamp(0.0, 1.0) * (vis::WIDE_BANDS - 1) as f32;
        let index = position.floor() as usize;
        let next = (index + 1).min(vis::WIDE_BANDS - 1);
        let between = position - index as f32;
        self.bands[index] * (1.0 - between) + self.bands[next] * between
    }
}

/// The loudness of `samples` and of their bass alone, as root mean squares.
fn loudness(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let (mut all, mut low, mut held) = (0.0f32, 0.0f32, 0.0f32);
    for sample in samples {
        held += BASS_PASS * (sample - held);
        all += sample * sample;
        low += held * held;
    }
    let count = samples.len() as f32;
    ((all / count).sqrt(), (low / count).sqrt())
}

/// `from` moved towards `to` by `rate`, never past it.
fn approach(from: f32, to: f32, rate: f32) -> f32 {
    from + (to - from) * rate.clamp(0.0, 1.0)
}

/// `from` following `to`: at `attack` a second when rising, `release` when
/// falling.
fn follow(from: f32, to: f32, dt: f32, attack: f32, release: f32) -> f32 {
    let rate = if to > from { attack } else { release };
    from + (to - from) * (1.0 - (-dt * rate).exp())
}

/// A steady pseudo-random number from 0 to 1 for `seed`.
fn hash(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// `color` as light added to whatever is behind it rather than painted
/// over it: premultiplied colour with no alpha of its own.
pub fn light(color: Color32, strength: f32) -> Color32 {
    let strength = strength.clamp(0.0, 1.0);
    let channel = |c: u8| (f32::from(c) * strength) as u8;
    Color32::from_rgba_premultiplied(
        channel(color.r()),
        channel(color.g()),
        channel(color.b()),
        0,
    )
}

/// The cover's colour bright and saturated enough to read as light, and
/// the same turned part of the way round the colour wheel to go with it.
pub fn lights(accent: Option<Color32>) -> (Color32, Color32) {
    let mut low = egui::ecolor::Hsva::from(accent.unwrap_or(Color32::from_rgb(120, 150, 255)));
    low.s = low.s.clamp(0.45, 0.85);
    low.v = 1.0;
    low.a = 1.0;
    let mut high = low;
    high.h = (high.h + 0.14).rem_euclid(1.0);
    high.s = (high.s * 0.8).max(0.4);
    (Color32::from(low), Color32::from(high))
}

/// Adds a soft blob to `mesh`: `color` in the middle, fading to nothing at
/// `radii`.
pub fn blob(mesh: &mut Mesh, center: Pos2, radii: Vec2, color: Color32) {
    let middle = mesh.vertices.len() as u32;
    let half = Color32::from_rgba_premultiplied(
        (f32::from(color.r()) * 0.38) as u8,
        (f32::from(color.g()) * 0.38) as u8,
        (f32::from(color.b()) * 0.38) as u8,
        (f32::from(color.a()) * 0.38) as u8,
    );
    mesh.colored_vertex(center, color);
    for segment in 0..BLOB_SEGMENTS {
        let theta = segment as f32 * std::f32::consts::TAU / BLOB_SEGMENTS as f32;
        let (sin, cos) = theta.sin_cos();
        let out = vec2(cos * radii.x, sin * radii.y);
        mesh.colored_vertex(center + out * 0.45, half);
        mesh.colored_vertex(center + out, Color32::TRANSPARENT);
    }
    for segment in 0..BLOB_SEGMENTS {
        let inner = middle + 1 + segment * 2;
        let outer = inner + 1;
        let next_inner = middle + 1 + ((segment + 1) % BLOB_SEGMENTS) * 2;
        let next_outer = next_inner + 1;
        mesh.add_triangle(middle, inner, next_inner);
        mesh.add_triangle(inner, outer, next_outer);
        mesh.add_triangle(inner, next_outer, next_inner);
    }
}

/// One soft blob of light, painted on its own.
pub fn paint_blob(painter: &egui::Painter, center: Pos2, radii: Vec2, color: Color32) {
    let mut mesh = Mesh::default();
    blob(&mut mesh, center, radii, color);
    painter.add(mesh);
}

/// A soft ring of `color`, `width` thick, fading to nothing on both sides.
fn ring(painter: &egui::Painter, center: Pos2, radius: f32, width: f32, color: Color32) {
    const SEGMENTS: u32 = 72;
    let mut mesh = Mesh::default();
    for segment in 0..SEGMENTS {
        let theta = segment as f32 * std::f32::consts::TAU / SEGMENTS as f32;
        let (sin, cos) = theta.sin_cos();
        let out = vec2(cos, sin);
        mesh.colored_vertex(
            center + out * (radius - width).max(0.0),
            Color32::TRANSPARENT,
        );
        mesh.colored_vertex(center + out * radius, color);
        mesh.colored_vertex(center + out * (radius + width), Color32::TRANSPARENT);
    }
    for segment in 0..SEGMENTS {
        let at = segment * 3;
        let next = ((segment + 1) % SEGMENTS) * 3;
        for band in 0..2 {
            mesh.add_triangle(at + band, at + band + 1, next + band + 1);
            mesh.add_triangle(at + band, next + band + 1, next + band);
        }
    }
    painter.add(mesh);
}

/// Darkness gathering towards the corners of `rect`, like a lens.
pub fn vignette(painter: &egui::Painter, rect: Rect, strength: f32) {
    const SEGMENTS: u32 = 48;
    let center = rect.center();
    let reach = rect.size() * 0.5 * std::f32::consts::SQRT_2;
    let edge = Color32::from_black_alpha((255.0 * strength.clamp(0.0, 1.0)) as u8);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, Color32::TRANSPARENT);
    for segment in 0..SEGMENTS {
        let theta = segment as f32 * std::f32::consts::TAU / SEGMENTS as f32;
        let (sin, cos) = theta.sin_cos();
        let out = vec2(cos * reach.x, sin * reach.y);
        mesh.colored_vertex(center + out * 0.55, Color32::TRANSPARENT);
        mesh.colored_vertex(center + out * 1.05, edge);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amplitude: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|n| (n as f32 * hz * std::f32::consts::TAU / 44_100.0).sin() * amplitude)
            .collect()
    }

    /// The bass filter hears a kick drum and not a cymbal.
    #[test]
    fn bass_is_the_low_end_only() {
        let (level, bass) = loudness(&tone(60.0, 0.5, 4_096));
        assert!(bass > level * 0.7, "{bass} of {level}");
        let (level, bass) = loudness(&tone(6_000.0, 0.5, 4_096));
        assert!(bass < level * 0.05, "{bass} of {level}");
        assert_eq!(loudness(&[]), (0.0, 0.0));
    }

    /// A kick after a quiet stretch is a beat, sends out a ring, and both
    /// fade away again; silence leaves the pulse at rest.
    #[test]
    fn a_kick_is_a_beat_that_fades() {
        let tap = vis::AudioTap::new();
        let mut fx = Fx::default();
        let push = |samples: &[f32]| {
            let stereo: Vec<f64> = samples
                .iter()
                .flat_map(|sample| [f64::from(*sample); 2])
                .collect();
            tap.push(&stereo, 1.0);
        };
        push(&tone(60.0, 0.005, 22_050));
        let mut time = 0.0;
        for _ in 0..30 {
            fx.update(&tap, true, true, time);
            time += 1.0 / 60.0;
        }
        assert!(fx.beat < 0.05, "a steady hum is no beat: {}", fx.beat);
        push(&tone(60.0, 0.8, 22_050));
        fx.update(&tap, true, true, time);
        assert_eq!(fx.beat, 1.0);
        assert_eq!(fx.rings.len(), 1);
        assert!(fx.moving());
        tap.clear();
        for _ in 0..600 {
            time += 1.0 / 60.0;
            fx.update(&tap, true, false, time);
        }
        assert!(fx.beat < 0.01 && fx.rings.is_empty());
        assert!(!fx.moving(), "silence while paused comes to rest");
    }

    /// A song playing on another device has no sound to read, so the
    /// pulse breathes gently rather than sitting dead.
    #[test]
    fn a_song_elsewhere_still_breathes() {
        let tap = vis::AudioTap::new();
        let mut fx = Fx::default();
        let mut time = 0.0;
        for _ in 0..240 {
            fx.update(&tap, false, true, time);
            time += 1.0 / 60.0;
        }
        assert!(fx.level > 0.05 && fx.level < 0.5, "{}", fx.level);
        assert_eq!(fx.beat, 0.0);
    }

    #[test]
    fn light_adds_and_never_covers() {
        let glow = light(Color32::from_rgb(200, 100, 50), 0.5);
        assert_eq!(glow.to_array(), [100, 50, 25, 0]);
        assert_eq!(light(Color32::WHITE, 0.0), Color32::TRANSPARENT);
    }

    #[test]
    fn the_lights_are_bright_whatever_the_cover() {
        for accent in [
            None,
            Some(Color32::from_rgb(20, 20, 22)),
            Some(Color32::WHITE),
        ] {
            let (low, high) = lights(accent);
            assert!(low.r().max(low.g()).max(low.b()) > 240);
            assert!(high.r().max(high.g()).max(high.b()) > 240);
            assert_ne!(low, high);
        }
    }
}
