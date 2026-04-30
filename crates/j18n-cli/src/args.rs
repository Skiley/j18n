use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
	name = "j18n",
	about = "Generate or sync localized i18n JSON files from a reference language using LLMs.",
	version
)]
pub struct Cli {
	#[command(subcommand)]
	pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
	#[command(about = "Create a skeleton JSON configuration file at the given path.")]
	Init(InitArgs),

	#[command(about = "Translate only missing entries or those changed since the last run.")]
	Sync(CommandArgs),

	#[command(about = "Translate every entry in the reference, replacing existing translations.")]
	Regenerate(CommandArgs),

	#[command(
		name = "check",
		about = "Report whether `sync` would translate or prune anything; exit non-zero if so."
	)]
	Check(CommandArgs),

	#[command(
		name = "baseline",
		about = "Record current reference hashes for each target without translating; useful when adopting j18n on a project with pre-existing translations so a follow-up `sync` doesn't re-translate everything."
	)]
	Baseline(CommandArgs),

	#[command(
		name = "install-git-hook",
		about = "Install a `pre-commit` hook in the current repo that runs `j18n check <CONFIGS>...`."
	)]
	InstallGitHook(CommandArgs),
}

#[derive(Args, Debug)]
pub struct InitArgs {
	#[arg(help = "Path where the skeleton config file will be written.")]
	pub path: PathBuf,
}

#[derive(Args, Debug)]
pub struct CommandArgs {
	#[arg(
		help = "One or more JSON configuration files describing what to translate.",
		num_args = 1..,
		required = true
	)]
	pub configs: Vec<PathBuf>,
}
