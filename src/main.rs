mod app;
mod event;
mod pane;
mod pty;
mod ui;

use clap::Parser;
use crossterm::style::{Color as CtColor, Print, ResetColor, SetForegroundColor};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "cdc", version, about = "Multi-session Claude Code orchestrator")]
struct Cli {
    #[arg(short, long)]
    n: Option<u32>,

    #[arg(long)]
    restore: Option<String>,

    #[arg(long)]
    cwd: Option<String>,

    #[arg(long)]
    setup: bool,
}

fn show_logo() -> std::io::Result<()> {
    let cyan = CtColor::Rgb {
        r: 0,
        g: 255,
        b: 255,
    };
    let dark_cyan = CtColor::Rgb {
        r: 0,
        g: 180,
        b: 180,
    };
    let dim = CtColor::Rgb {
        r: 100,
        g: 100,
        b: 100,
    };

    let border = dark_cyan;
    let body = cyan;

    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, Print("\n"))?;

    // Top border
    crossterm::execute!(
        stdout,
        SetForegroundColor(border),
        Print("  \u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}\n"),
        Print("  \u{2502}                                                  \u{2502}\n"),
    )?;

    // CDC ASCII art
    let art = [
        "    \u{2591}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2557}\u{2591}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2557}\u{2591}\u{2591}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2557}\u{2591}",
        "    \u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2557}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2557}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2557}",
        "    \u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{255a}\u{2550}\u{255d}\u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{255a}\u{2550}\u{255d}",
        "    \u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{2588}\u{2588}\u{2557}\u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}\u{2591}\u{2591}\u{2588}\u{2588}\u{2557}",
        "    \u{255a}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{255a}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}",
        "    \u{2591}\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}\u{2591}\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}\u{2591}\u{2591}\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}\u{2591}",
    ];
    for line in &art {
        crossterm::execute!(
            stdout,
            SetForegroundColor(border),
            Print("  \u{2502}"),
            SetForegroundColor(body),
            Print(format!("{:<50}", line)),
            SetForegroundColor(border),
            Print("\u{2502}\n"),
        )?;
    }

    // Subtitle
    crossterm::execute!(
        stdout,
        SetForegroundColor(border),
        Print("  \u{2502}                                                  \u{2502}\n"),
        Print("  \u{2502}"),
        SetForegroundColor(dim),
        Print("    c l a u d e - d e v e l o p m e n t - c l i   "),
        SetForegroundColor(border),
        Print("\u{2502}\n"),
        Print("  \u{2502}                                                  \u{2502}\n"),
        // Bottom border
        Print("  \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}\n"),
        ResetColor,
    )?;

    crossterm::execute!(
        stdout,
        Print("\n"),
        SetForegroundColor(dim),
        Print("  v0.1.0 \u{00b7} Multi-session Claude Code orchestrator\n"),
        Print("  for GPU-accelerated terminals\n\n"),
        ResetColor,
    )?;

    std::thread::sleep(Duration::from_secs(1));
    Ok(())
}

fn main() {
    let _cli = Cli::parse();

    if let Err(e) = show_logo() {
        eprintln!("Logo display error: {e}");
    }

    if let Err(e) = app::run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
