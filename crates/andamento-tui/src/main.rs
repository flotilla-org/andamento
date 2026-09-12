#[cfg(target_family = "wasm")]
fn main() {}

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::env;
    use std::fs;
    use std::io::{self, BufRead, Write};
    use std::process::{Command, ExitCode, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use andamento_core::sidebar::{HostEffect, Request, Sidebar, Workspace};
    use andamento_core::MetadataPatch;
    use andamento_terminal::surface;
    use crossterm::{cursor, event, execute, terminal};

    struct TmuxHost {
        target: String,
        workspaces: Vec<Workspace>,
    }

    impl TmuxHost {
        fn observe(&mut self) -> io::Result<Vec<Workspace>> {
            let output = Command::new("tmux")
                .args([
                    "list-windows",
                    "-t",
                    &self.target,
                    "-F",
                    "#{window_id}\t#{window_index}\t#{window_name}\t#{window_active}",
                ])
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other("tmux list-windows failed"));
            }
            self.workspaces = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split('\t');
                    Some(Workspace {
                        id: fields.next()?.trim_start_matches('@').parse().ok()?,
                        position: fields.next()?.parse().ok()?,
                        name: fields.next()?.to_owned(),
                        selected: fields.next()? == "1",
                    })
                })
                .collect();
            Ok(self.workspaces.clone())
        }

        fn execute(&mut self, effect: HostEffect) -> io::Result<(u64, Option<u64>)> {
            match effect {
                HostEffect::Focus {
                    request_id,
                    workspace_id,
                } => {
                    let position = self
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id == workspace_id)
                        .ok_or_else(|| io::Error::other("tmux window disappeared"))?
                        .position;
                    tmux_status([
                        "select-window",
                        "-t",
                        &format!("{}:{position}", self.target),
                    ])?;
                    Ok((request_id, None))
                }
                HostEffect::Materialize {
                    request_id,
                    name,
                    recipe,
                    cwd,
                    ..
                } => {
                    let mut command = Command::new("tmux");
                    command.args([
                        "new-window",
                        "-P",
                        "-F",
                        "#{window_id}",
                        "-t",
                        &self.target,
                        "-n",
                        &name,
                    ]);
                    if let Some(cwd) = cwd.as_deref() {
                        command.args(["-c", cwd]);
                    }
                    command.arg(recipe);
                    let output = command.output()?;
                    if !output.status.success() {
                        return Err(io::Error::other("tmux new-window failed"));
                    }
                    let id = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .trim_start_matches('@')
                        .parse()
                        .map_err(|_| io::Error::other("tmux returned an invalid window id"))?;
                    Ok((request_id, Some(id)))
                }
                HostEffect::Inspect { .. } => Err(io::Error::other("inspect is not implemented")),
            }
        }
    }

    fn tmux_status<const N: usize>(args: [&str; N]) -> io::Result<()> {
        let status = Command::new("tmux").args(args).status()?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| io::Error::other(format!("tmux exited with {status}")))
    }

    pub fn main() -> ExitCode {
        run().map(|()| ExitCode::SUCCESS).unwrap_or_else(|error| {
            eprintln!("andamento-tui: {error}");
            ExitCode::FAILURE
        })
    }

    fn run() -> io::Result<()> {
        let mut args = env::args().skip(1);
        let target = args.next().ok_or_else(|| {
            io::Error::other("usage: andamento-tui TMUX_TARGET CONFIG_KDL FACTS_COMMAND...")
        })?;
        let config = args
            .next()
            .ok_or_else(|| io::Error::other("missing CONFIG_KDL"))?;
        let facts_command = args.collect::<Vec<_>>();
        let mut sidebar = Sidebar::new(&fs::read_to_string(config)?).map_err(io::Error::other)?;
        let (sender, receiver) = mpsc::channel();
        spawn_facts_reader(facts_command, sender)?;
        let mut host = TmuxHost {
            target,
            workspaces: vec![],
        };
        terminal::enable_raw_mode()?;
        let mut output = io::stdout();
        execute!(output, terminal::EnterAlternateScreen, cursor::Hide)?;
        let result = event_loop(&mut sidebar, &mut host, &receiver, &mut output);
        execute!(output, cursor::Show, terminal::LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        result
    }

    fn spawn_facts_reader(
        command: Vec<String>,
        sender: mpsc::Sender<MetadataPatch>,
    ) -> io::Result<()> {
        let program = command
            .first()
            .ok_or_else(|| io::Error::other("missing FACTS_COMMAND"))?;
        let mut child = Command::new(program)
            .args(&command[1..])
            .stdout(Stdio::piped())
            .spawn()?;
        std::thread::spawn(move || {
            for line in io::BufReader::new(child.stdout.take().unwrap())
                .lines()
                .map_while(Result::ok)
            {
                if let Ok(patch) = serde_json::from_str(&line) {
                    let _ = sender.send(patch);
                }
            }
            let _ = child.wait();
        });
        Ok(())
    }

    fn event_loop(
        sidebar: &mut Sidebar,
        host: &mut TmuxHost,
        facts: &mpsc::Receiver<MetadataPatch>,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let mut selected = 0usize;
        loop {
            let patches = facts.try_iter().collect::<Vec<_>>();
            sidebar.apply(now_ms(), patches);
            sidebar.observe(host.observe()?, vec![]);
            let frame = surface::render(&sidebar.snapshot().surface, terminal::size()?.0 as usize);
            selected = selected.min(frame.hits.len().saturating_sub(1));
            draw(&frame.lines, selected, output)?;
            if event::poll(Duration::from_millis(200))? {
                if let event::Event::Key(key) = event::read()? {
                    match key.code {
                        event::KeyCode::Char('q') => return Ok(()),
                        event::KeyCode::Up => selected = selected.saturating_sub(1),
                        event::KeyCode::Down => {
                            selected = (selected + 1).min(frame.hits.len().saturating_sub(1))
                        }
                        event::KeyCode::Enter => {
                            if let Some(hit) = frame.hits.get(selected) {
                                let response = sidebar
                                    .handle(Request::Dispatch {
                                        action: hit.action.clone(),
                                    })
                                    .map_err(io::Error::other)?;
                                for effect in response.effects {
                                    if let Ok((request_id, workspace_id)) = host.execute(effect) {
                                        sidebar
                                            .handle(Request::Complete {
                                                request_id,
                                                workspace_id,
                                                error: None,
                                            })
                                            .map_err(io::Error::other)?;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn draw(lines: &[String], selected: usize, output: &mut impl Write) -> io::Result<()> {
        execute!(
            output,
            cursor::MoveTo(0, 0),
            terminal::Clear(terminal::ClearType::All)
        )?;
        for (row, line) in lines.iter().enumerate() {
            writeln!(
                output,
                "{} {line}\r",
                if row == selected { '>' } else { ' ' }
            )?;
        }
        write!(output, "\r\n[↑/↓] select  [enter] activate  [q] quit")?;
        output.flush()
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[cfg(not(target_family = "wasm"))]
fn main() -> std::process::ExitCode {
    native::main()
}
