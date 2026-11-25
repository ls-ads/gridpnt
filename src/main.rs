use std::path::Path;
use std::process::ExitCode;

use gridpnt::backend::sqlite::Sqlite;
use gridpnt::tui;

const USAGE: &str = "\
gridpnt — a terminal-native database tool

Usage:
  gridpnt <file>      open a SQLite database (read-only)
  gridpnt --help      show this help
  gridpnt --version   show the version

Keys:
  j k h l             move                gg G    first / last row
  Ctrl-d Ctrl-u       half page           0 $     first / last column
  l / h  (catalog)    open / close        Enter   preview table
  Tab, Ctrl-w h/l     switch pane         :       run a query
  Ctrl-c              cancel query        q       quit";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let path = match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("gridpnt {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [path] if !path.starts_with('-') => Path::new(path),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let backend = match Sqlite::open(path) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("gridpnt: {error}");
            return ExitCode::FAILURE;
        }
    };

    match tui::run(Box::new(backend)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gridpnt: {error}");
            ExitCode::FAILURE
        }
    }
}
