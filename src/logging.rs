use crate::cli::{Cli, ColorChoice};
use owo_colors::OwoColorize;
use std::io::{Write, stderr, stdout};
use time::{OffsetDateTime, macros::format_description};

pub fn init(args: &Cli) {
    // Colors handled per-line; nothing global to init for now.
    if matches!(args.color, ColorChoice::Never) {
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
    }
}

pub fn ts() -> String {
    let now = OffsetDateTime::now_utc();
    let fmt = format_description!("[hour]:[minute]:[second].[subsecond digits:3]Z");
    now.format(&fmt).unwrap_or_else(|_| "--:--:--.---Z".into())
}

pub fn println_tag(tag: &str, s: &str) {
    let child_raw = std::env::var("B2B_CHILD_RAW").ok().as_deref() == Some("1");
    if child_raw {
        // Child processes emit raw lines; orchestrator adds timestamp + role.
        let mut e = stderr().lock();
        let _ = writeln!(e, "{}", s);
        let _ = e.flush();
        return;
    }
    let t = ts();
    let line = if std::env::var("NO_COLOR").is_ok() {
        format!("[{t}] {tag} {s}")
    } else {
        format!("{} {} {}", format!("[{t}]").dimmed(), tag.bold(), s)
    };
    let mut e = stderr().lock();
    let _ = writeln!(e, "{}", line);
    let _ = e.flush();
}

pub fn role_tag(role: &str) -> String {
    // Color palette aligned with HLD.
    match role.to_ascii_lowercase().as_str() {
        "orchestrator" => {
            if std::env::var("NO_COLOR").is_ok() {
                "[ORCH]".into()
            } else {
                "[ORCH]".yellow().to_string()
            }
        }
        "source" => {
            if std::env::var("NO_COLOR").is_ok() {
                "[SRC ]".into()
            } else {
                "[SRC ]".green().to_string()
            }
        }
        "mixer" => {
            if std::env::var("NO_COLOR").is_ok() {
                "[MIX ]".into()
            } else {
                "[MIX ]".magenta().to_string()
            }
        }
        "sink" => {
            if std::env::var("NO_COLOR").is_ok() {
                "[SINK]".into()
            } else {
                "[SINK]".blue().to_string()
            }
        }
        _ => "[b2b]".into(),
    }
}

pub fn ready_line(role: &str, sip: &str, codec: &str, ptime_ms: u32) {
    // READY lines are machine-parsed by the orchestrator; keep them on stdout but flush.
    let mut o = stdout().lock();
    let _ = writeln!(
        o,
        "READY role={role} sip={sip} codec={codec} ptime={ptime_ms}ms"
    );
    let _ = o.flush();
}
