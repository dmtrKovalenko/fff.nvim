use clap::Args;
use fff_ipc_domain::CaseMode;

#[derive(Args, Debug)]
#[group(required = false, multiple = false)]
pub struct CaseModeArgs {
    #[arg(short = 'i', long = "ignore-case")]
    ignore_case: bool,

    #[arg(short = 's', long = "case-sensitive")]
    case_sensitive: bool,

    #[arg(short = 'S', long = "smart-case")]
    smart_case: bool,
}

impl CaseModeArgs {
    pub fn resolve(&self) -> CaseMode {
        match (self.case_sensitive, self.ignore_case) {
            (true, _) => CaseMode::Sensitive,
            (_, true) => CaseMode::Insensitive,
            _ => CaseMode::Smart,
        }
    }

    pub fn apply_to_rg(&self, cmd: &mut std::process::Command) {
        if self.ignore_case {
            cmd.arg("-i");
        } else if self.case_sensitive {
            cmd.arg("-s");
        } else {
            cmd.arg("-S");
        }
    }
}
