use andamento_core::{sidebar::Request, MetadataPatch, Sidebar};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: andamento-html <config.kdl> <patches.jsonl> [requests.jsonl]".into());
    }
    let mut sidebar = Sidebar::new(&std::fs::read_to_string(&args[0])?)?;
    let patches = std::fs::read_to_string(&args[1])?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str::<MetadataPatch>)
        .collect::<Result<Vec<_>, _>>()?;
    sidebar.apply(0, patches);
    if let Some(path) = args.get(2) {
        for line in std::fs::read_to_string(path)?
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            sidebar.handle(serde_json::from_str::<Request>(line)?)?;
        }
    }
    println!("{}", andamento_html::render(&sidebar.snapshot()));
    Ok(())
}
