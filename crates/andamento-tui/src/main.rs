use std::env;
use std::io::{self, BufRead, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use andamento_core::render::{render_lines, LocalTab};
use andamento_core::{Facts, HostControl, SidebarCore};
use crossterm::{cursor, event, execute, terminal};

struct TmuxHost {
    target: String,
}

impl HostControl for TmuxHost {
    type Error = io::Error;

    fn switch_tab(&mut self, position: usize) -> io::Result<()> {
        run_tmux([
            "select-window",
            "-t",
            &format!("{}:{}", self.target, position),
        ])
    }

    fn open_tab(&mut self, name: &str) -> io::Result<()> {
        run_tmux(["new-window", "-t", &self.target, "-n", name])
    }
}

fn run_tmux<const N: usize>(args: [&str; N]) -> io::Result<()> {
    let status = Command::new("tmux").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("tmux exited with {status}")))
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("andamento-tui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let target = args.next().unwrap_or_else(|| ".".to_owned());
    let facts_command = args.collect::<Vec<_>>();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || -> io::Result<()> {
        let mut child = facts_command
            .first()
            .map(|program| {
                Command::new(program)
                    .args(&facts_command[1..])
                    .stdout(Stdio::piped())
                    .spawn()
            })
            .transpose()?;
        let reader: Box<dyn BufRead> = match child.as_mut() {
            Some(child) => Box::new(io::BufReader::new(child.stdout.take().unwrap())),
            None => Box::new(io::BufReader::new(io::stdin())),
        };
        for line in reader.lines().map_while(Result::ok) {
            let facts = serde_json::from_str::<Facts>(&line)
                .or_else(|_| serde_json::from_str(&line).map(|model| Facts::ViewModel { model }));
            if let Ok(facts) = facts {
                let _ = sender.send(facts);
            }
        }
        if let Some(mut child) = child {
            child.wait()?;
        }
        Ok(())
    });

    let mut core = SidebarCore::default();
    let mut host = TmuxHost { target };
    terminal::enable_raw_mode()?;
    let mut output = io::stdout();
    execute!(output, terminal::EnterAlternateScreen, cursor::Hide)?;
    let result = event_loop(&mut core, &mut host, &receiver, &mut output);
    execute!(output, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

fn event_loop(
    core: &mut SidebarCore,
    host: &mut TmuxHost,
    receiver: &mpsc::Receiver<Facts>,
    output: &mut impl Write,
) -> io::Result<()> {
    loop {
        while let Ok(facts) = receiver.try_recv() {
            core.apply(facts);
        }
        draw(core, output)?;
        if event::poll(Duration::from_millis(100))? {
            if let event::Event::Key(key) = event::read()? {
                match key.code {
                    event::KeyCode::Char('q') => return Ok(()),
                    event::KeyCode::Char('n') => host.open_tab("andamento")?,
                    event::KeyCode::Char(digit @ '1'..='9') => {
                        let index = digit.to_digit(10).unwrap() as usize - 1;
                        if let Some(tab) = core.tabs().get(index) {
                            core.activate(tab.id, host)?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw(core: &SidebarCore, output: &mut impl Write) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let tabs = core
        .tabs()
        .iter()
        .map(|tab| LocalTab {
            tab_id: tab.id,
            position: tab.position,
            name: tab.name.clone(),
            active: tab.active,
        })
        .collect::<Vec<_>>();
    let rendered = render_lines(
        core.model(),
        &tabs,
        rows as usize,
        cols as usize,
        core.model().is_some(),
    );
    execute!(
        output,
        cursor::MoveTo(0, 0),
        terminal::Clear(terminal::ClearType::All)
    )?;
    write!(
        output,
        "{}\r\n\r\n[1-9] switch tmux window  [n] new window  [q] quit",
        rendered.lines.join("\r\n")
    )?;
    output.flush()
}
