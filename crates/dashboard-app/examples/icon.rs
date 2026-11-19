//! Draws Project Dashboard's mark (`src/mark.rs`) as a PNG at any size, or as SVG. The Windows executable embeds the
//! same mark as its icon (`build.rs`).
//!
//! `cargo run -p dashboard-app --example icon -- <out.png> <size>`
//! `cargo run -p dashboard-app --example icon -- <out.svg>`

use image::RgbaImage;

#[path = "../src/mark.rs"]
mod mark;

fn icon(size: u32) -> RgbaImage {
    RgbaImage::from_raw(size, size, mark::rgba(size)).expect("the mark has size x size pixels")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        usage();
    };
    let written = if out.ends_with(".svg") {
        std::fs::write(&out, mark::svg()).map_err(|err| err.to_string())
    } else {
        let Some(size) = args.next().and_then(|size| size.parse::<u32>().ok()) else {
            usage();
        };
        icon(size).save(&out).map_err(|err| err.to_string())
    };
    if let Err(err) = written {
        eprintln!("icon: couldn't write {out}: {err}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: icon <out.png> <size> | icon <out.svg>");
    std::process::exit(2);
}
