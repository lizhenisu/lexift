use std::{env, fs, path::PathBuf};

use resvg::{tiny_skia, usvg};

const CURSOR_SIZES: [u32; 3] = [32, 48, 64];
const SUPERSAMPLE: u32 = 4;

fn main() {
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/annotation-corner-radius.svg");
    println!("cargo:rerun-if-changed={}", source.display());

    let svg = fs::read_to_string(&source).expect("read annotation cursor SVG");
    let tree =
        usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("parse annotation cursor SVG");
    assert!(
        (tree.size().width() - tree.size().height()).abs() < f32::EPSILON,
        "annotation cursor SVG must have a square view box"
    );

    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for size in CURSOR_SIZES {
        let rgba = render_cursor(&tree, size);
        fs::write(
            output.join(format!("annotation-corner-radius-{size}.rgba")),
            rgba,
        )
        .expect("write annotation cursor raster");
    }
}

/// Render at four times the target size and average premultiplied pixels.
/// Keeping them premultiplied preserves smooth, fringe-free edges in Win32.
fn render_cursor(tree: &usvg::Tree, size: u32) -> Vec<u8> {
    let high_size = size * SUPERSAMPLE;
    let mut pixmap =
        tiny_skia::Pixmap::new(high_size, high_size).expect("allocate annotation cursor pixmap");
    let scale = high_size as f32 / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let source = pixmap.data();
    let mut output = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut channels = [0u32; 4];
            for dy in 0..SUPERSAMPLE {
                for dx in 0..SUPERSAMPLE {
                    let offset =
                        (((y * SUPERSAMPLE + dy) * high_size + x * SUPERSAMPLE + dx) * 4) as usize;
                    for (sum, sample) in channels.iter_mut().zip(&source[offset..offset + 4]) {
                        *sum += u32::from(*sample);
                    }
                }
            }
            let samples = SUPERSAMPLE * SUPERSAMPLE;
            output.extend(channels.map(|sum| ((sum + samples / 2) / samples) as u8));
        }
    }
    output
}
