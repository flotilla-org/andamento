#[cfg(target_family = "wasm")]
fn main() {}

#[cfg(not(target_family = "wasm"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use andamento_git_watcher::{
        refresh::{factory_layout, run_cycle, RefreshWatch},
        transport::{Stdout, Transport, Wheelhouse, Zellij},
    };
    use std::{
        env,
        path::PathBuf,
        time::{Duration, Instant},
    };
    let mut roots = Vec::new();
    let mut mode = "stdout".to_owned();
    let mut socket = env::var_os("WHEELHOUSE_SOCKET").map(PathBuf::from);
    let mut bin = env::var("ZELLIJ_BIN").unwrap_or_else(|_| "zellij".into());
    let mut plugin = None;
    let mut factory = false;
    let mut layout = None;
    let mut interval = 5.0_f64;
    let mut ttl_ms = 10_000_u64;
    let mut once = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--roots" => roots.push(PathBuf::from(
                args.next().ok_or("--roots needs a directory")?,
            )),
            "--transport" => {
                mode = args
                    .next()
                    .ok_or("--transport needs stdout, zellij or wheelhouse")?
            }
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or("--socket needs a path")?)),
            "--zellij-bin" => bin = args.next().ok_or("--zellij-bin needs a path")?,
            "--plugin-url" => plugin = Some(args.next().ok_or("--plugin-url needs a URL")?),
            "--factory-repo-manager" => factory = true,
            "--factory-layout" => {
                layout = Some(PathBuf::from(
                    args.next().ok_or("--factory-layout needs a path")?,
                ))
            }
            "--interval" => interval = args.next().ok_or("--interval needs seconds")?.parse()?,
            "--ttl-ms" => ttl_ms = args.next().ok_or("--ttl-ms needs milliseconds")?.parse()?,
            "--once" => once = true,
            "--help" | "-h" => {
                println!("andamento-git-watcher [--roots DIR]... [--transport stdout|zellij|wheelhouse] [--socket PATH] [--once] [--interval SECONDS] [--ttl-ms MS]\nZellij: [--zellij-bin PATH] [--plugin-url URL] [--factory-repo-manager] [--factory-layout PATH]\nDefaults: stdout JSONL, interval 5s, TTL 10000ms. Requires git; Wheelhouse also requires curl.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if !interval.is_finite() || interval <= 0.0 || interval * 1000.0 >= ttl_ms as f64 {
        return Err("interval must be positive and shorter than TTL".into());
    }
    if factory && mode != "zellij" {
        return Err("--factory-repo-manager requires --transport zellij".into());
    }
    if mode == "stdout" && roots.is_empty() {
        return Err("stdout discovery requires at least one --roots directory".into());
    }
    for root in &roots {
        if !root.is_dir() {
            return Err(format!("not a directory: {}", root.display()).into());
        }
    }
    let layout = factory_layout(factory, layout)?;
    let mut transport: Box<dyn Transport> = match mode.as_str() {
        "stdout" => Box::new(Stdout),
        "wheelhouse" => Box::new(Wheelhouse {
            socket: socket.ok_or("--socket or WHEELHOUSE_SOCKET required")?,
        }),
        "zellij" => Box::new(Zellij {
            bin,
            plugin,
            factory_layout: layout,
        }),
        _ => return Err("unknown transport".into()),
    };
    let interval = Duration::from_secs_f64(interval);
    loop {
        let start = Instant::now();
        let trees = match run_cycle(&roots, transport.as_mut(), ttl_ms) {
            Ok(cycle) => {
                for error in &cycle.errors {
                    eprintln!("andamento-git-watcher: {error}");
                }
                if once && !cycle.errors.is_empty() {
                    return Err(cycle.errors.join("; ").into());
                }
                cycle.trees
            }
            Err(error) if once => return Err(error.into()),
            Err(error) => {
                eprintln!("andamento-git-watcher: {error}");
                Vec::new()
            }
        };
        if start.elapsed() >= Duration::from_millis(ttl_ms) {
            eprintln!("andamento-git-watcher: refresh took {:?}, exceeding TTL {ttl_ms}ms; increase --ttl-ms for this inventory", start.elapsed());
        }
        if once {
            return Ok(());
        }
        // Cheap local invalidation between full TTL refreshes. Working-file
        // edits and host inventory changes are picked up by the regular loop.
        let watch = RefreshWatch::new(&trees);
        while start.elapsed() < interval {
            std::thread::sleep(
                Duration::from_millis(250).min(interval.saturating_sub(start.elapsed())),
            );
            if watch.changed() {
                break;
            }
        }
    }
}
