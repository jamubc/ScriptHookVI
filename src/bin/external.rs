use std::path::PathBuf;

fn main() {
    let mut names = Vec::new();
    let mut overlay = false;
    let mut run = None;
    let mut wait = 0;
    let home = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let mut scripts = std::env::var_os("SCRIPTHOOKVI_SCRIPTS").map_or_else(|| home.join("scripts"), PathBuf::from);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--overlay" => overlay = true,
            "--run" => run = args.next().map(PathBuf::from),
            "--wait" => wait = args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--scripts" => scripts = args.next().map(PathBuf::from).unwrap_or(scripts),
            "--help" | "-h" => {
                println!("usage: ScriptHookVI-external [--overlay] [--scripts <folder>] [--run <file.lua> [--wait <seconds>]] <process.exe | pid>...");
                return;
            }
            _ => names.push(arg),
        }
    }
    if names.is_empty() {
        eprintln!("ScriptHookVI-external: name a process or a pid");
        std::process::exit(2);
    }
    if let Some(file) = run {
        std::process::exit(ScriptHookVI::engine::probe(&names, &file, std::time::Duration::from_secs(wait)));
    }
    ScriptHookVI::set_home(home);
    ScriptHookVI::engine::run(scripts, ScriptHookVI::engine::Mode::External { names, overlay });
}
