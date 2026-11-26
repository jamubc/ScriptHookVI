use std::io::Write;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: preview <scripts folder> <out.rgba> <width> <height> [lua run after the scripts load]");
        std::process::exit(2);
    }
    let size = (args[2].parse().unwrap_or(1280), args[3].parse().unwrap_or(720));
    let code = args.get(4).map_or("menu.open = true", String::as_str);
    let pixels = ScriptHookVI::engine::preview(Path::new(&args[0]), size, code, 3);
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        let a = p >> 24;
        let un = |shift: u32| if a == 0 { 0 } else { (((p >> shift) & 0xff) * 255 / a).min(255) as u8 };
        out.extend([un(16), un(8), un(0), a as u8]);
    }
    std::fs::File::create(&args[1]).and_then(|mut f| f.write_all(&out)).expect("write the picture");
}
