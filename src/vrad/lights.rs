//! Light sources: light / light_spot / light_environment entities and texture lights
//! (lights.rad), with Source's intensity and falloff conventions. Intensities are linear,
//! where 1.0 is full brightness (worldlight lump units).

use super::scene::{get, Scene};
use crate::bspfile::DWorldLight;
use crate::math::angles_matrix;
use glam::DVec3;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Surface = 0,
    Point = 1,
    Spot = 2,
    Sky = 3,
    SkyAmbient = 5,
}

#[derive(Clone, Debug)]
pub struct Light {
    pub kind: Kind,
    pub origin: DVec3,
    pub intensity: DVec3,
    /// Spot / sun: direction the light travels. Surface: emitter normal.
    pub normal: DVec3,
    pub cluster: i32,
    pub style: u8,
    pub stopdot: f64,
    pub stopdot2: f64,
    pub exponent: f64,
    /// Cut-off distance (0 = none).
    pub radius: f64,
    pub constant: f64,
    pub linear: f64,
    pub quadratic: f64,
    /// Sun spread (cosine of the half angle) for soft shadows; 1 = hard.
    pub sun_spread: f64,
}

impl Light {
    /// Attenuation at distance `d` (1/(c + l d + q d^2)).
    pub fn falloff(&self, d: f64) -> f64 {
        let denom = self.constant + self.linear * d + self.quadratic * d * d;
        if denom <= 1e-9 { 1.0 } else { 1.0 / denom }
    }
}

/// `_light`: "r g b brightness" with gamma 2.2 colours (result in 0..255 * brightness/255).
fn light_for_string(s: &str) -> DVec3 {
    let v: Vec<f64> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    let (r, g, b, scale) = match v.len() {
        0 => (0.0, 0.0, 0.0, 255.0),
        1 => (v[0], v[0], v[0], 255.0),
        2 | 3 => (v[0], v.get(1).copied().unwrap_or(v[0]), v.get(2).copied().unwrap_or(v[0]), 255.0),
        _ => (v[0], v[1], v[2], v[3]),
    };
    let lin = |c: f64| (c.max(0.0) / 255.0).powf(2.2) * 255.0;
    DVec3::new(lin(r), lin(g), lin(b)) * (scale / 255.0)
}

fn num(e: &[(String, String)], k: &str) -> f64 {
    get(e, k).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)
}

fn vec3(e: &[(String, String)], k: &str) -> Option<DVec3> {
    get(e, k).and_then(crate::vmf::parse_vec3)
}

/// Direction from angles + `pitch` (Hammer's pitch is negated for lights).
fn light_direction(e: &[(String, String)]) -> DVec3 {
    let mut a = vec3(e, "angles").unwrap_or(DVec3::ZERO);
    if let Some(p) = get(e, "pitch").and_then(|v| v.trim().parse::<f64>().ok()) {
        a.x = -p;
    } else {
        a.x = -a.x;
    }
    if let Some(y) = get(e, "angle").and_then(|v| v.trim().parse::<f64>().ok()) {
        if vec3(e, "angles").is_none() {
            a.y = y;
        }
    }
    angles_matrix(a).x_axis
}

fn solve_falloff(d50: f64, d0: f64) -> (f64, f64, f64) {
    // 1/(c + b d + a d^2) through (0, 1), (d50, 1/2), (d0, 1/256).
    let c = 1.0;
    // c + b d50 + a d50^2 = 2 ; c + b d0 + a d0^2 = 256
    let (r1, r2) = (2.0 - c, 256.0 - c);
    let det = d50 * d0 * d0 - d0 * d50 * d50;
    if det.abs() < 1e-9 {
        return (0.0, 0.0, 1.0);
    }
    let b = (r1 * d0 * d0 - r2 * d50 * d50) / det;
    let a = (d50 * r2 - d0 * r1) / det;
    if a < 0.0 || b < 0.0 {
        // Not monotonic: fall back to pure quadratic through the 50% point.
        return (1.0 / (d50 * d50), 0.0, 1.0);
    }
    (a, b, c)
}

pub struct LightSet {
    pub lights: Vec<Light>,
    pub sun: Option<usize>,
    /// Texture light emission per material (lowercase name).
    pub texlights: HashMap<String, DVec3>,
}

pub fn parse_rad(text: &str, out: &mut HashMap<String, DVec3>) {
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        let mut it = line.split_whitespace();
        let Some(name) = it.next() else { continue };
        let vals: Vec<&str> = it.collect();
        if vals.len() < 3 {
            continue;
        }
        let s = vals[..vals.len().min(4)].join(" ");
        out.insert(crate::material::normalize_name(name), light_for_string(&s));
    }
}

/// Builds all entity lights; named lights keep their `style`.
pub fn collect(scene: &Scene, texlights: HashMap<String, DVec3>) -> LightSet {
    let mut lights = Vec::new();
    let mut sun = None;
    for e in &scene.entities {
        let class = get(e, "classname").unwrap_or("").to_ascii_lowercase();
        if !matches!(class.as_str(), "light" | "light_spot" | "light_environment") {
            continue;
        }
        let origin = vec3(e, "origin").unwrap_or(DVec3::ZERO);
        let style = num(e, "style").clamp(0.0, 63.0) as u8;
        let cluster = scene.cluster_at(origin);
        if class == "light_environment" {
            let dir = light_direction(e);
            let mut intensity = light_for_string(get(e, "_light").unwrap_or("255 255 255 200")) / 255.0;
            if intensity.max_element() > 0.0 {
                let spread = num(e, "SunSpreadAngle");
                sun = Some(lights.len());
                lights.push(Light {
                    kind: Kind::Sky,
                    origin,
                    intensity,
                    normal: dir,
                    cluster,
                    style,
                    stopdot: 0.0,
                    stopdot2: 0.0,
                    exponent: 0.0,
                    radius: 0.0,
                    constant: 1.0,
                    linear: 0.0,
                    quadratic: 0.0,
                    sun_spread: if spread > 0.0 { (spread.to_radians() / 2.0).cos() } else { 1.0 },
                });
            }
            intensity = light_for_string(get(e, "_ambient").unwrap_or("0 0 0 0")) / 255.0;
            if intensity.max_element() > 0.0 {
                lights.push(Light {
                    kind: Kind::SkyAmbient,
                    origin,
                    intensity,
                    normal: DVec3::NEG_Z,
                    cluster,
                    style,
                    stopdot: 0.0,
                    stopdot2: 0.0,
                    exponent: 0.0,
                    radius: 0.0,
                    constant: 1.0,
                    linear: 0.0,
                    quadratic: 0.0,
                    sun_spread: 1.0,
                });
            }
            continue;
        }
        let mut intensity = light_for_string(get(e, "_light").unwrap_or("255 255 255 200"));
        let (mut c, mut l, mut q) = (num(e, "_constant_attn").max(0.0), num(e, "_linear_attn").max(0.0), num(e, "_quadratic_attn").max(0.0));
        let d50 = num(e, "_fifty_percent_distance");
        if d50 > 0.0 {
            let mut d0 = num(e, "_zero_percent_distance");
            if d0 < d50 {
                d0 = 2.0 * d50;
            }
            (q, l, c) = solve_falloff(d50, d0);
        } else if c < 1e-6 && l < 1e-6 && q < 1e-6 {
            c = 1.0;
        }
        let ratio = c + 100.0 * l + 10000.0 * q;
        if ratio > 0.0 && d50 <= 0.0 {
            intensity *= ratio;
        }
        intensity /= 255.0;
        let mut light = Light {
            kind: Kind::Point,
            origin,
            intensity,
            normal: DVec3::X,
            cluster,
            style,
            stopdot: 0.0,
            stopdot2: 0.0,
            exponent: 0.0,
            radius: num(e, "_distance").max(0.0),
            constant: c,
            linear: l,
            quadratic: q,
            sun_spread: 1.0,
        };
        if class == "light_spot" {
            let mut inner = num(e, "_inner_cone");
            if inner == 0.0 {
                inner = 10.0;
            }
            let mut outer = num(e, "_cone");
            if outer == 0.0 {
                outer = inner;
            }
            outer = outer.max(inner);
            light.kind = Kind::Spot;
            light.normal = light_direction(e);
            light.stopdot = inner.to_radians().cos();
            light.stopdot2 = outer.to_radians().cos();
            light.exponent = num(e, "_exponent");
        }
        lights.push(light);
    }
    LightSet { lights, sun, texlights }
}

/// WORLDLIGHTS lump entries.
pub fn world_lights(set: &LightSet) -> Vec<DWorldLight> {
    set.lights
        .iter()
        .map(|l| DWorldLight {
            origin: l.origin.into(),
            intensity: l.intensity.into(),
            normal: l.normal.into(),
            shadow_cast_offset: Default::default(),
            cluster: l.cluster,
            kind: l.kind as i32,
            style: l.style as i32,
            stopdot: l.stopdot as f32,
            stopdot2: l.stopdot2 as f32,
            exponent: l.exponent as f32,
            radius: l.radius as f32,
            constant_attn: l.constant as f32,
            linear_attn: l.linear as f32,
            quadratic_attn: l.quadratic as f32,
            flags: 2,
            texinfo: 0,
            owner: 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_intensity_convention() {
        // "_light" "255 255 255 200" with quadratic falloff -> 200 * 10000 / 255 (Valve's lump).
        let i = light_for_string("255 255 255 200") * 10000.0 / 255.0;
        assert!((i.x - 7843.137).abs() < 0.01);
        let (a, b, c) = solve_falloff(100.0, 2000.0);
        let f = |d: f64| 1.0 / (c + b * d + a * d * d);
        assert!((f(0.0) - 1.0).abs() < 1e-9 && (f(100.0) - 0.5).abs() < 1e-9 && (f(2000.0) - 1.0 / 256.0).abs() < 1e-9);
        // No monotonic fit: pure quadratic through the 50% point.
        let (a, b, _) = solve_falloff(100.0, 300.0);
        assert!(b == 0.0 && (1.0 / (1.0 + a * 1e4) - 0.5).abs() < 1e-9);
    }
}
