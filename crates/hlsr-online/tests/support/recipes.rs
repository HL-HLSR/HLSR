//! Картинки корпусов по рецепту: тот же генератор, что `tools/reference/online/lib/recipes.js`
//! (xorshift32, байт — старшие 8 бит состояния).

#![allow(dead_code)]

use serde_json::Value;

fn xorshift(seed: u32) -> impl FnMut() -> u8 {
    let mut s = if seed == 0 { 1 } else { seed };
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 24) as u8
    }
}

/// RGBA по рецепту `{kind: "noise" | "sprite", …}`.
pub fn generate(recipe: &Value) -> Vec<u8> {
    let wide = recipe["wide"].as_u64().unwrap() as usize;
    let tall = recipe["tall"].as_u64().unwrap() as usize;
    let mut out = vec![0u8; wide * tall * 4];
    match recipe["kind"].as_str().unwrap() {
        "noise" => {
            let mut next = xorshift(recipe["seed"].as_u64().unwrap() as u32);
            let alpha = recipe["alpha"].as_str().unwrap();
            for i in 0..wide * tall {
                let (r, g, b, a) = (next(), next(), next(), next());
                let px = match alpha {
                    "opaque" => [r, g, b, 255],
                    "binary" if a < 128 => [0, 0, 0, 0],
                    "binary" => [r, g, b, 255],
                    "premul" => [r.min(a), g.min(a), b.min(a), a],
                    _ => [r, g, b, a],
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
        }
        "sprite" => {
            let mut next = xorshift(recipe["seed"].as_u64().unwrap() as u32);
            let shade = f64::from(next());
            let (w, h) = (wide as f64, tall as f64);
            for y in 0..tall {
                for x in 0..wide {
                    let (xf, yf) = (x as f64, y as f64);
                    let inside =
                        xf >= w / 4.0 && xf < w * 3.0 / 4.0 && yf >= h / 4.0 && yf < h * 3.0 / 4.0;
                    let edge = x.min(y).min(wide - 1 - x).min(tall - 1 - y) as f64;
                    let px = if inside {
                        [
                            (xf * 255.0) / (w - 1.0).max(1.0),
                            (yf * 255.0) / (h - 1.0).max(1.0),
                            shade,
                            255.0,
                        ]
                    } else {
                        [
                            shade,
                            255.0 - (xf * 255.0) / (w - 1.0).max(1.0),
                            (yf * 255.0) / (h - 1.0).max(1.0),
                            if edge < 2.0 { 90.0 + edge * 70.0 } else { 0.0 },
                        ]
                    };
                    for (k, v) in px.iter().enumerate() {
                        out[(y * wide + x) * 4 + k] = v.clamp(0.0, 255.0).floor() as u8;
                    }
                }
            }
        }
        other => panic!("неизвестный рецепт {other}"),
    }
    out
}
