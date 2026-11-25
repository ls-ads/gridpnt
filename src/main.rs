use std::path::Path;
use std::process::ExitCode;

use gridpnt::backend::Backend;
use gridpnt::backend::databricks::{Databricks, auth};
use gridpnt::backend::sqlite::Sqlite;
use gridpnt::config::{self, Connection, Kind};
use gridpnt::tui;

const USAGE: &str = "\
gridpnt — a terminal-native database tool

Usage:
  gridpnt <connection>   open a connection from connections.ini
  gridpnt <file>         open a SQLite database file
  gridpnt --help         show this help
  gridpnt --version      show the version

Everything is opened read-only.

Keys:
  j k h l             move                gg G    first / last row
  Ctrl-d Ctrl-u       half page           0 $     first / last column
  l / h  (catalog)    open / close        Enter   preview table
  Tab, Ctrl-w h/l     switch pane         :       run a query
  Ctrl-c              cancel query        q       quit";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let target = match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            print_connections();
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("gridpnt {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [target] if !target.starts_with('-') => target,
        _ => {
            eprintln!("{USAGE}");
            print_connections();
            return ExitCode::FAILURE;
        }
    };

    let outcome = open(target).and_then(|(backend, env)| tui::run(backend, env).map_err(|e| e.to_string()));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gridpnt: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Open `target`: a named connection if there is one, otherwise a file.
fn open(target: &str) -> Result<(Box<dyn Backend>, Option<String>), String> {
    let connections = config::load_connections()?;
    let Some(connection) = connections.into_iter().find(|c| c.name == target) else {
        return Ok((Box::new(Sqlite::open(Path::new(target))?), None));
    };
    let Connection { name, env, kind } = connection;
    let backend: Box<dyn Backend> = match kind {
        Kind::Sqlite { path } => Box::new(Sqlite::open(&path)?),
        Kind::Databricks(settings) => {
            let credentials = auth::from_system(&settings)?;
            Box::new(Databricks::new(name, credentials, settings.warehouse, &settings.pins))
        }
    };
    Ok((backend, env))
}

fn print_connections() {
    let Some(file) = config::config_dir().map(|dir| dir.join("connections.ini")) else {
        return;
    };
    match config::load_connections() {
        Ok(connections) if connections.is_empty() => {
            println!("\nNo connections yet. Define them in {}", file.display())
        }
        Ok(connections) => {
            println!("\nConnections ({}):", file.display());
            for connection in connections {
                println!("  {}", connection.name);
            }
        }
        Err(error) => println!("\n{error}"),
    }
}
