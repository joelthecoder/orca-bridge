//! Human terminal styling. Structured output never calls these renderers.
use clap::{
    ColorChoice,
    builder::{
        Styles,
        styling::{AnsiColor, Effects},
    },
};
use serde_json::Value;
use std::{
    io::IsTerminal,
    sync::atomic::{AtomicBool, Ordering},
};

static COLOR: AtomicBool = AtomicBool::new(false);

pub fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
}

pub fn color_choice() -> ColorChoice {
    // Decide before Clap renders help. An explicit flag wins over environment defaults.
    let args: Vec<_> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate().rev() {
        let value = arg.strip_prefix("--color=").or_else(|| {
            (arg == "--color")
                .then(|| args.get(i + 1).map(String::as_str))
                .flatten()
        });
        if let Some(value) = value {
            match value {
                "always" => return ColorChoice::Always,
                "never" => return ColorChoice::Never,
                _ => {}
            }
        }
    }
    if std::env::var_os("NO_COLOR").is_some() || std::env::var("TERM").as_deref() == Ok("dumb") {
        return ColorChoice::Never;
    }
    match std::env::var("FORCE_COLOR").as_deref() {
        Ok("0") => ColorChoice::Never,
        Ok(s) if !s.is_empty() => ColorChoice::Always,
        _ => ColorChoice::Auto,
    }
}

pub fn configure(choice: ColorChoice) {
    COLOR.store(
        match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => std::io::stdout().is_terminal(),
        },
        Ordering::Relaxed,
    );
}

pub fn paint(value: &str, code: &str) -> String {
    let value: String = value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect();
    if COLOR.load(Ordering::Relaxed) {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value
    }
}

pub fn session(value: &Value) -> String {
    let field = |key: &str| value[key].as_str().unwrap_or("");
    format!(
        "{}\n  {} · {} · {}\n  {}\n",
        paint(field("title"), "1;36"),
        paint(field("agent"), "33"),
        paint(value["model"].as_str().unwrap_or("unknown model"), "2"),
        paint(field("cwd"), "2"),
        paint(field("id"), "2")
    )
}
