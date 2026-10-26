//! `nexo`: Nexo CMS from the terminal.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod cli;
mod cmd;
mod ctx;
mod out;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use ctx::Ctx;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("błąd: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut ctx = Ctx::new(cli.profile, cli.json)?;
    match cli.command {
        Command::Login(args) => cmd::account::login(&mut ctx, args).await,
        Command::Logout => cmd::account::logout(&ctx).await,
        Command::Status => cmd::account::status(&ctx).await,
        Command::Profile(command) => cmd::account::profile(&mut ctx, command),
        Command::Sessions(command) => cmd::account::sessions(&ctx, command).await,
        Command::Api(args) => cmd::account::api(&ctx, args).await,
        Command::Ops { filter } => cmd::ops::list(&ctx, filter).await,
        Command::Call(args) => cmd::ops::call(&ctx, args).await,
        Command::Sites(command) => cmd::content::sites(&ctx, command).await,
        Command::Docs(command) => cmd::content::docs(&ctx, command).await,
        Command::Media(command) => cmd::content::media(&ctx, command).await,
        Command::Menus(command) => cmd::structure::menus(&ctx, command).await,
        Command::Terms(command) => cmd::structure::terms(&ctx, command).await,
        Command::Redirects(command) => cmd::structure::redirects(&ctx, command).await,
        Command::Layouts(command) => cmd::structure::layouts(&ctx, command).await,
        Command::Theme(command) => cmd::structure::theme(&ctx, command).await,
        Command::Extensions(command) => cmd::structure::extensions(&ctx, command).await,
        Command::Forms(command) => cmd::forms::forms(&ctx, command).await,
        Command::Collections(command) => cmd::forms::collections(&ctx, command).await,
        Command::Shop(command) => cmd::shop::run(&ctx, command).await,
        Command::Users(command) => cmd::people::users(&ctx, command).await,
        Command::Groups(command) => cmd::people::groups(&ctx, command).await,
        Command::Audit(args) => cmd::people::audit(&ctx, args).await,
        Command::Images(command) => cmd::images::run(&ctx, command).await,
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn command_tree_is_valid() {
        super::Cli::command().debug_assert();
    }
}
