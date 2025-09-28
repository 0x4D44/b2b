pub mod source;
pub mod sink;
pub mod mixer;

pub trait RoleRun {
    fn run(&self) -> anyhow::Result<()>;
}

