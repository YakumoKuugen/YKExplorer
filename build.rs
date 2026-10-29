//! Builds the Windows icon for the executable from `assets/icons/explorer.svg`.
//!
//! The SVG is rasterised at several sizes, packed into an `.ico`, then compiled
//! into a `.res` with the Windows resource compiler and linked into the binary
//! so Explorer / the taskbar show it.

use std::{env, fs, path::PathBuf, process::Command};

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/explorer.svg");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let svg_path = manifest.join("assets").join("icons").join("explorer.svg");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));

    let Ok(svg) = fs::read_to_string(&svg_path) else {
        println!("cargo:warning=icon svg missing, skipping executable icon");
        return;
    };

    let Some(ico) = build_ico(&svg) else {
        println!("cargo:warning=could not rasterise the icon");
        return;
    };
    let ico_path = out_dir.join("fpilot.ico");
    if fs::write(&ico_path, &ico).is_err() {
        println!("cargo:warning=could not write fpilot.ico");
        return;
    }

    // 1 ICON "<path>" -- the numeric id becomes the default application icon.
    let rc_path = out_dir.join("fpilot.rc");
    let escaped = ico_path.display().to_string().replace('\\', "\\\\");
    if fs::write(&rc_path, format!("1 ICON \"{escaped}\"\n")).is_err() {
        println!("cargo:warning=could not write the resource script");
        return;
    }

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found, executable icon not embedded");
        return;
    };
    let res_path = out_dir.join("fpilot.res");
    let status = Command::new(rc)
        .arg("/nologo")
        .arg(format!("/fo{}", res_path.display()))
        .arg(&rc_path)
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
        other => println!("cargo:warning=rc.exe failed: {other:?}"),
    }
}

fn build_ico(svg: &str) -> Option<Vec<u8>> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &options).ok()?;

    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for size in SIZES {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        images.push((size, pixmap.encode_png().ok()?));
    }

    let count = images.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        let dimension = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dimension);
        out.push(dimension);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        out.extend_from_slice(png);
    }
    Some(out)
}

fn find_rc() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = fs::read_dir(&kits) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("x64").join("rc.exe");
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort();
    if let Some(best) = candidates.pop() {
        return Some(best);
    }
    // Fall back to PATH (a developer command prompt).
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    None
}
