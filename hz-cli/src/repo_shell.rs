use std::io::{self, IsTerminal};

use crate::{InitArgs, ShellArg, ShellArgs, render_repo_init, render_shell_init};
use hz_core::HzResult;

pub(crate) fn init_repo_or_shell(args: InitArgs) -> HzResult<()> {
    if let Some(shell) = args.shell {
        if args.repo.is_some() {
            return Err(hz_core::HzError::Usage(
                "hz init <shell> does not accept --repo; use hz install <shell>".to_owned(),
            ));
        }
        return install_shell(ShellArgs { shell });
    }

    let init = hz_command::init_repo(hz_command::InitRepo { repo: args.repo })?;
    print!("{}", render_repo_init(&init, io::stdout().is_terminal()));

    Ok(())
}

pub(crate) fn install_shell(args: ShellArgs) -> HzResult<()> {
    let shell = shell_to_command(args.shell);

    let init = hz_command::install_shell_integration(shell)?;
    print!(
        "{}",
        render_shell_init(shell_name(args.shell), &init, io::stdout().is_terminal())
    );

    Ok(())
}

pub(crate) fn shell_script(args: ShellArgs) -> HzResult<()> {
    let shell = shell_to_command(args.shell);

    print!("{}", hz_command::shell_integration(shell));
    Ok(())
}

pub(crate) fn shell_to_command(shell: ShellArg) -> hz_command::Shell {
    match shell {
        ShellArg::Zsh => hz_command::Shell::Zsh,
        ShellArg::Bash => hz_command::Shell::Bash,
        ShellArg::Fish => hz_command::Shell::Fish,
    }
}

pub(crate) fn shell_name(shell: ShellArg) -> &'static str {
    match shell {
        ShellArg::Zsh => "zsh",
        ShellArg::Bash => "bash",
        ShellArg::Fish => "fish",
    }
}
