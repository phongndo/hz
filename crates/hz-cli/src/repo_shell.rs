use crate::{
    CliResult,
    args::{ShellArg, ShellArgs},
    write_stdout,
};

pub(crate) fn install_shell(args: ShellArgs) -> CliResult<()> {
    let shell = shell_to_command(args.shell);
    let initialized = hz_command::install_shell_integration(shell)?;
    write_stdout(format_args!(
        "{} {}\n",
        if initialized.changed {
            "installed"
        } else {
            "already installed"
        },
        initialized.path.display()
    ))?;
    Ok(())
}

pub(crate) fn shell_script(args: ShellArgs) -> CliResult<()> {
    write_stdout(format_args!(
        "{}",
        hz_command::shell_integration(shell_to_command(args.shell))
    ))?;
    Ok(())
}

fn shell_to_command(shell: ShellArg) -> hz_command::Shell {
    match shell {
        ShellArg::Zsh => hz_command::Shell::Zsh,
        ShellArg::Bash => hz_command::Shell::Bash,
        ShellArg::Fish => hz_command::Shell::Fish,
    }
}
