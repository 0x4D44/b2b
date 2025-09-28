use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RoleKind {
    Orchestrator,
    Source,
    Sink,
    Mixer,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogFormat { Text, Json }

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ColorChoice { Auto, Always, Never }

#[derive(Parser, Debug)]
#[command(name = "b2b", version, about = "SIP/RTP audio roles and orchestrator", disable_help_subcommand = true)]
pub struct Cli {
    /// Role to run
    #[arg(long, value_enum)]
    pub role: RoleKind,

    /// SIP bind address for sink/mixer
    #[arg(long, value_name = "IP:PORT")]
    pub sip_bind: Option<String>,

    /// Outbound SIP target for source/mixer
    #[arg(long, value_name = "SIP-URI")]
    pub target: Option<String>,

    /// Log output format for child processes
    #[arg(long, default_value = "text", value_enum)]
    pub log_format: LogFormat,

    /// Color choice for console output
    #[arg(long, default_value = "auto", value_enum)]
    pub color: ColorChoice,

    // Source
    #[arg(long, value_name = "FILE")]
    pub audio_file: Option<PathBuf>,

    #[arg(long, default_value_t = 1000)]
    pub prebuffer_ms: u32,

    /// Delay after call established before sending media
    #[arg(long, default_value_t = 120)]
    pub preroll_ms: u32,

    #[arg(long, default_value_t = 20)]
    pub ptime_ms: u32,

    #[arg(long, default_value = "pcmu")]
    pub codec: String,

    // Sink
    #[arg(long, default_value = "aplay -f S16_LE -r 8000 -c 1 -t raw")]
    pub aplay_cmd: String,

    // Mixer
    #[arg(long, default_value = "123#")]
    pub dtmf_seq: String,

    #[arg(long, default_value_t = 2000)]
    pub dtmf_period_ms: u32,

    #[arg(long, default_value_t = 0.5)]
    pub mix_gain_in: f32,

    #[arg(long, default_value_t = 0.5)]
    pub mix_gain_dtmf: f32,

    // Orchestrator
    #[arg(long, value_name = "FILE")]
    pub plan: Option<PathBuf>,

    #[arg(long, default_value_t = 5000)]
    pub grace_ms: u64,

    #[arg(long, default_value_t = 15000)]
    pub kill_ms: u64,

    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Orchestrator readiness wait timeout
    #[arg(long, default_value_t = 10000)]
    pub ready_ms: u64,
}
