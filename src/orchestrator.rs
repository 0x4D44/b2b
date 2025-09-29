use crate::{
    cli::{Cli, RoleKind},
    logging, util,
};
use anyhow::{Context, Result};
use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
struct PlanTopology {
    source_target: Option<String>,
    source_audio_file: Option<PathBuf>,
    source_preroll_ms: Option<u32>,
    mixer_bind: Option<String>,
    mixer_target: Option<String>,
    mixer_dtmf_seq: Option<String>,
    mixer_dtmf_period_ms: Option<u32>,
    mixer_gain_in: Option<f32>,
    mixer_gain_dtmf: Option<f32>,
    sink_bind: Option<String>,
    sink_aplay_cmd: Option<String>,
    sink_buffer_min_ms: Option<u32>,
    sink_buffer_max_ms: Option<u32>,
    sink_buffer_mode: Option<String>,
    sink_jbuf_min_ms: Option<u32>,
    sink_jbuf_max_ms: Option<u32>,
    sink_jbuf_type: Option<String>,
}

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("orchestrator");
    logging::println_tag(&tag, "starting");

    if let Some(plan_path) = &args.plan {
        let topo = load_plan(plan_path)?;
        if args.dry_run {
            print_plan_cmds(&topo, args);
            return Ok(());
        }
        let mut children = Vec::<(RoleKind, Child)>::new();
        let (tx, rx) = mpsc::channel::<String>();

        // 1) Spawn sink first and wait READY
        if let Some(bind) = topo.sink_bind.as_deref() {
            let mut extra = vec!["--sip-bind".to_string(), bind.to_string()];
            if let Some(ap) = topo.sink_aplay_cmd.as_deref() {
                extra.push("--aplay-cmd".into());
                extra.push(ap.into());
            }
            if let (Some(min), Some(max)) = (topo.sink_buffer_min_ms, topo.sink_buffer_max_ms) {
                extra.push("--sink-buffer-min-ms".into());
                extra.push(min.to_string());
                extra.push("--sink-buffer-max-ms".into());
                extra.push(max.to_string());
            }
            if let Some(mode) = topo.sink_buffer_mode.as_deref() {
                extra.push("--sink-buffer-mode".into());
                extra.push(mode.into());
            }
            if let (Some(min), Some(max)) = (topo.sink_jbuf_min_ms, topo.sink_jbuf_max_ms) {
                extra.push("--sink-jbuf-min-ms".into());
                extra.push(min.to_string());
                extra.push("--sink-jbuf-max-ms".into());
                extra.push(max.to_string());
            }
            if let Some(jt) = topo.sink_jbuf_type.as_deref() {
                extra.push("--sink-jbuf-type".into());
                extra.push(jt.into());
            }
            let (role, mut ch) = spawn_role(RoleKind::Sink, &extra, args)?;
            pipe_child_output(&role, &mut ch, tx.clone());
            children.push((role, ch));
            let mut expected: HashSet<&'static str> = HashSet::new();
            expected.insert("sink");
            wait_for_ready(&expected, args.ready_ms, &rx);
        }

        // 2) Spawn mixer next (it dials sink), then wait READY
        if let (Some(bind), Some(target)) =
            (topo.mixer_bind.as_deref(), topo.mixer_target.as_deref())
        {
            let mut extra: Vec<String> = vec![
                "--sip-bind".into(),
                bind.into(),
                "--target".into(),
                target.into(),
            ];
            if let Some(seq) = topo.mixer_dtmf_seq.as_deref() {
                extra.push("--dtmf-seq".into());
                extra.push(seq.into());
            }
            if let Some(p) = topo.mixer_dtmf_period_ms {
                extra.push("--dtmf-period-ms".into());
                extra.push(p.to_string());
            }
            if let Some(g) = topo.mixer_gain_in {
                extra.push("--mix-gain-in".into());
                extra.push(format!("{:.3}", g));
            }
            if let Some(g) = topo.mixer_gain_dtmf {
                extra.push("--mix-gain-dtmf".into());
                extra.push(format!("{:.3}", g));
            }
            let (role, mut ch) = spawn_role(RoleKind::Mixer, &extra, args)?;
            pipe_child_output(&role, &mut ch, tx.clone());
            children.push((role, ch));
            let mut expected: HashSet<&'static str> = HashSet::new();
            expected.insert("mixer");
            wait_for_ready(&expected, args.ready_ms, &rx);
        }

        // 3) Spawn source after sink and mixer are ready
        if let Some(target) = topo.source_target.as_deref() {
            let mut extra: Vec<String> = vec!["--target".into(), target.into()];
            if let Some(audio) = topo.source_audio_file.as_deref() {
                let s = audio.to_string_lossy().to_string();
                extra.push("--audio-file".into());
                extra.push(s);
            }
            let (role, mut ch) = spawn_role(RoleKind::Source, &extra, args)?;
            pipe_child_output(&role, &mut ch, tx.clone());
            children.push((role, ch));
        }

        // 4) Monitor for early child exit or Ctrl-C
        monitor_or_ctrlc(&tag, &mut children, args.grace_ms, args.kill_ms);
    }
    // Wait for ctrl-c then exit children on future pass; skeleton exits immediately without children alive.
    Ok(())
}

fn load_plan(path: &PathBuf) -> Result<PlanTopology> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("reading plan: {}", path.display()))?;
    let v: toml::Value = toml::from_str(&s).context("parsing plan TOML")?;
    let mut topo = PlanTopology::default();
    if let Some(t) = v.get("topology") {
        topo.source_target = t
            .get("source")
            .and_then(|s| s.get("sip_target"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.source_audio_file = t
            .get("source")
            .and_then(|s| s.get("audio_file"))
            .and_then(|x| x.as_str())
            .map(PathBuf::from);
        topo.source_preroll_ms = t
            .get("source")
            .and_then(|s| s.get("preroll_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.mixer_bind = t
            .get("mixer")
            .and_then(|m| m.get("sip_bind"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.mixer_target = t
            .get("mixer")
            .and_then(|m| m.get("sip_target"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.mixer_dtmf_seq = t
            .get("mixer")
            .and_then(|m| m.get("dtmf_seq"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.mixer_dtmf_period_ms = t
            .get("mixer")
            .and_then(|m| m.get("dtmf_period_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.mixer_gain_in = t
            .get("mixer")
            .and_then(|m| m.get("mix_gain_in"))
            .and_then(|x| x.as_float())
            .map(|v| v as f32);
        topo.mixer_gain_dtmf = t
            .get("mixer")
            .and_then(|m| m.get("mix_gain_dtmf"))
            .and_then(|x| x.as_float())
            .map(|v| v as f32);
        topo.sink_bind = t
            .get("sink")
            .and_then(|m| m.get("sip_bind"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.sink_aplay_cmd = t
            .get("sink")
            .and_then(|m| m.get("aplay_cmd"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.sink_buffer_min_ms = t
            .get("sink")
            .and_then(|m| m.get("buffer_min_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.sink_buffer_max_ms = t
            .get("sink")
            .and_then(|m| m.get("buffer_max_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.sink_buffer_mode = t
            .get("sink")
            .and_then(|m| m.get("buffer_mode"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        topo.sink_jbuf_min_ms = t
            .get("sink")
            .and_then(|m| m.get("jbuf_min_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.sink_jbuf_max_ms = t
            .get("sink")
            .and_then(|m| m.get("jbuf_max_ms"))
            .and_then(|x| x.as_integer())
            .map(|v| v as u32);
        topo.sink_jbuf_type = t
            .get("sink")
            .and_then(|m| m.get("jbuf_type"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
    }
    // Replace placeholders like YOUR_HOST_IP / YOUR_IP in plan values
    if let Some(ip) = detect_host_ipv4() {
        let replace = |opt: &mut Option<String>| {
            if let Some(v) = opt.as_mut()
                && (v.contains("YOUR_HOST_IP") || v.contains("YOUR_IP"))
            {
                *v = v.replace("YOUR_HOST_IP", &ip).replace("YOUR_IP", &ip);
            }
        };
        replace(&mut topo.source_target);
        replace(&mut topo.mixer_bind);
        replace(&mut topo.mixer_target);
        replace(&mut topo.sink_bind);
    }
    Ok(topo)
}

fn detect_host_ipv4() -> Option<String> {
    use std::net::{IpAddr, UdpSocket};
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    let _ = sock.connect(("8.8.8.8", 80));
    if let Ok(addr) = sock.local_addr()
        && let IpAddr::V4(ip4) = addr.ip()
    {
        return Some(ip4.to_string());
    }
    let _ = sock.connect(("1.1.1.1", 80));
    if let Ok(addr) = sock.local_addr()
        && let IpAddr::V4(ip4) = addr.ip()
    {
        return Some(ip4.to_string());
    }
    None
}

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("current_exe")
}

fn spawn_role(
    role: RoleKind,
    extra: &[String],
    args: &Cli,
) -> Result<(RoleKind, std::process::Child)> {
    let exe = exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--role").arg(role_str(role));
    // pass consistent output options
    cmd.args([
        "--log-format",
        match args.log_format {
            crate::cli::LogFormat::Text => "text",
            crate::cli::LogFormat::Json => "json",
        },
    ]);
    cmd.args([
        "--color",
        match args.color {
            crate::cli::ColorChoice::Auto => "auto",
            crate::cli::ColorChoice::Always => "always",
            crate::cli::ColorChoice::Never => "never",
        },
    ]);
    // Ensure children emit raw lines; we add timestamp + [ROLE] here.
    cmd.env("B2B_CHILD_RAW", "1");
    cmd.args(extra);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .with_context(|| format!("spawning role {}", role_str(role)))?;
    Ok((role, child))
}

fn role_str(r: RoleKind) -> &'static str {
    match r {
        RoleKind::Orchestrator => "orchestrator",
        RoleKind::Source => "source",
        RoleKind::Sink => "sink",
        RoleKind::Mixer => "mixer",
    }
}

fn print_plan_cmds(topo: &PlanTopology, _args: &Cli) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "b2b".into());
    let orch = logging::role_tag("orchestrator");
    if let Some(bind) = topo.sink_bind.as_deref() {
        let mut cmd = format!("dry-run: {exe} --role sink --sip-bind {bind}");
        if let Some(ap) = topo.sink_aplay_cmd.as_deref() {
            cmd.push_str(&format!(" --aplay-cmd \"{}\"", ap));
        }
        logging::println_tag(&orch, &cmd);
    }
    if let (Some(bind), Some(target)) = (topo.mixer_bind.as_deref(), topo.mixer_target.as_deref()) {
        logging::println_tag(
            &orch,
            &format!("dry-run: {exe} --role mixer --sip-bind {bind} --target {target}"),
        );
    }
    if let Some(target) = topo.source_target.as_deref() {
        let af = topo
            .source_audio_file
            .as_ref()
            .map(|p| format!(" --audio-file {}", p.display()))
            .unwrap_or_default();
        logging::println_tag(
            &orch,
            &format!("dry-run: {exe} --role source --target {target}{af}"),
        );
    }
}

// (unused old helpers removed)

fn shutdown_children(children: &mut [(RoleKind, Child)], grace_ms: u64, kill_ms: u64) {
    // Send SIGTERM first
    for (_, ch) in children.iter_mut() {
        let id = ch.id();
        unsafe {
            libc::kill(id as i32, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_millis(grace_ms);
    while Instant::now() < deadline {
        let mut all_done = true;
        for (_, ch) in children.iter_mut() {
            match ch.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    all_done = false;
                }
                Err(_) => {}
            }
        }
        if all_done {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Escalate to SIGKILL
    let kdeadline = Instant::now() + Duration::from_millis(kill_ms.saturating_sub(grace_ms));
    for (_, ch) in children.iter_mut() {
        let id = ch.id();
        unsafe {
            libc::kill(id as i32, libc::SIGKILL);
        }
    }
    while Instant::now() < kdeadline {
        let mut all_done = true;
        for (_, ch) in children.iter_mut() {
            match ch.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    all_done = false;
                }
                Err(_) => {}
            }
        }
        if all_done {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn monitor_or_ctrlc(tag: &str, children: &mut [(RoleKind, Child)], grace_ms: u64, kill_ms: u64) {
    // Listen for Ctrl-C
    let (ctx, crx) = mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || {
        let _ = ctx.send(());
    });

    loop {
        // Check for child exit
        for (role, child) in children.iter_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let role = *role;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(code) = status.code() {
                        logging::println_tag(
                            tag,
                            &format!(
                                "child {role:?} exited with code {code}; shutting down others"
                            ),
                        );
                    } else if let Some(sig) = status.signal() {
                        logging::println_tag(
                            tag,
                            &format!(
                                "child {role:?} terminated by signal {} ; shutting down others",
                                sig
                            ),
                        );
                    } else {
                        logging::println_tag(
                            tag,
                            &format!("child {role:?} exited; shutting down others"),
                        );
                    }
                }
                #[cfg(not(unix))]
                {
                    let code = status.code().unwrap_or(-1);
                    logging::println_tag(
                        tag,
                        &format!("child {role:?} exited with code {code}; shutting down others"),
                    );
                }
                shutdown_children(children, grace_ms, kill_ms);
                return;
            }
        }
        // Ctrl-C
        if crx.try_recv().is_ok() {
            logging::println_tag(tag, "Ctrl+C received; shutting down children");
            shutdown_children(children, grace_ms, kill_ms);
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn pipe_child_output(role: &RoleKind, child: &mut Child, tx: mpsc::Sender<String>) {
    let (out, err) = util::child_pipes(child);
    let tag = logging::role_tag(match role {
        RoleKind::Sink => "sink",
        RoleKind::Source => "source",
        RoleKind::Mixer => "mixer",
        RoleKind::Orchestrator => "orchestrator",
    });
    let txo = tx.clone();
    let tag_stdout = tag.clone();
    if let Some(o) = out {
        util::spawn_reader_thread(o, tag.clone(), move |_tag, line| {
            if let Some(role) = parse_ready_role(line) {
                let _ = txo.send(role.to_string());
            }
            logging::println_tag(&tag_stdout, line);
        });
    }
    if let Some(e) = err {
        let tag_stderr = tag.clone();
        util::spawn_reader_thread(e, tag.clone(), move |_tag, line| {
            if let Some(role) = parse_ready_role(line) {
                let _ = tx.send(role.to_string());
            }
            logging::println_tag(&tag_stderr, line);
        });
    }
}

fn parse_ready_role(line: &str) -> Option<&str> {
    if !line.starts_with("READY ") {
        return None;
    }
    // Expect fields like: READY role=<role> ...
    for tok in line.split_whitespace() {
        if let Some(val) = tok.strip_prefix("role=") {
            return Some(val);
        }
    }
    None
}

fn wait_for_ready(expected: &HashSet<&'static str>, ready_ms: u64, rx: &mpsc::Receiver<String>) {
    if expected.is_empty() {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(ready_ms);
    let mut seen: HashSet<String> = HashSet::new();
    while Instant::now() < deadline {
        if let Ok(role) = rx.recv_timeout(Duration::from_millis(100)) {
            seen.insert(role);
            if expected.iter().all(|r| seen.contains(*r)) {
                break;
            }
        }
    }
}
