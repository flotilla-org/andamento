use andamento_core::{
    replay::{self, Recorder, Replay},
    Sidebar,
};
use std::{
    env,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match args.as_slice() {
        [mode] if mode == "record" => {
            let mut recorder = Recorder::new(out);
            for line in io::stdin().lock().lines() {
                let line = line?;
                if !line.trim().is_empty() { recorder.record(serde_json::from_str(&line)?)?; }
            }
        }
        [mode, path] if mode == "play" || mode == "emit" => {
            let frames = replay::read(BufReader::new(File::open(path)?))?;
            let start = Instant::now();
            for frame in frames {
                if mode == "play" { std::thread::sleep(Duration::from_millis(frame.offset_ms).saturating_sub(start.elapsed())); }
                serde_json::to_writer(&mut out, &replay::wire_patch(&frame.patch))?;
                writeln!(out)?;
                out.flush()?;
            }
        }
        [mode, path, config, at] if mode == "snapshot" => {
            let mut replay = Replay::new(replay::read(BufReader::new(File::open(path)?))?)?;
            let mut sidebar = Sidebar::new(&std::fs::read_to_string(config)?)?;
            replay.advance_to(&mut sidebar, at.parse()?)?;
            serde_json::to_writer(&mut out, &sidebar.snapshot())?;
            writeln!(out)?;
        }
        _ => return Err("usage: andamento-replay record < patches.jsonl | play|emit CAPTURE | snapshot CAPTURE CONFIG_KDL OFFSET_MS".into()),
    }
    Ok(())
}
